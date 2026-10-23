//! The module's logic, free of any Logos plumbing so it can be unit-tested
//! with plain `cargo test`.
//!
//! Everything returns JSON values or `Err(message)`; the module layer turns
//! those into JSON strings (`{"error": "..."}` on failure).

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_dyn_abi::{DynSolValue, FunctionExt, JsonAbiExt, Specifier};
use alloy_eips::eip2718::Encodable2718;
use alloy_json_abi::JsonAbi;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use serde_json::{json, Value};
use solizone_evm::{
    embedded::{EmbeddedConfig, EmbeddedNode},
    node::dev_accounts::DEV_ACCOUNTS,
};

use crate::solc;

const DEPLOY_GAS: u64 = 3_000_000;
const SEND_GAS: u64 = 1_000_000;
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Default)]
pub struct Engine {
    node: Option<EmbeddedNode>,
    solc: Option<PathBuf>,
}

impl Engine {
    fn node(&self) -> Result<&EmbeddedNode, String> {
        self.node
            .as_ref()
            .ok_or_else(|| "the chain is not running; call start first".to_string())
    }

    /// Starts the chain in `data_dir` (or recovers the one stored there).
    pub fn start(&mut self, data_dir: &str) -> Result<Value, String> {
        if let Some(node) = &self.node {
            return Ok(describe(node));
        }

        if data_dir.trim().is_empty() {
            return Err("start needs a data directory".to_string());
        }

        let node = EmbeddedNode::start(EmbeddedConfig {
            block_time: Duration::from_millis(500),
            ..EmbeddedConfig::local(data_dir)
        })?;

        let info = describe(&node);
        self.node = Some(node);

        Ok(info)
    }

    pub fn stop(&mut self) -> Value {
        let was_running = self.node.take().map(EmbeddedNode::stop).is_some();

        json!({ "stopped": was_running })
    }

    pub fn status(&self) -> Value {
        match &self.node {
            Some(node) => describe(node),
            None => json!({ "running": false }),
        }
    }

    /// The pre-funded dev accounts. The keys are public test keys, used for
    /// signing inside the module; they are never returned.
    pub fn accounts(&self) -> Value {
        let accounts: Vec<Value> = DEV_ACCOUNTS
            .iter()
            .enumerate()
            .map(|(index, account)| {
                json!({ "index": index, "address": account.address.to_string() })
            })
            .collect();

        json!({ "accounts": accounts })
    }

    pub fn compile(&mut self, source: &str) -> Result<Value, String> {
        if self.solc.is_none() {
            self.solc = Some(solc::find_solc()?);
        }

        solc::compile(self.solc.as_ref().expect("set above"), source)
    }

    /// One raw JSON-RPC request in, one response out.
    pub fn rpc(&self, request: &str) -> String {
        match self.node() {
            Ok(node) => node.request(request),
            Err(message) => json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32000, "message": message },
            })
            .to_string(),
        }
    }

    /// Deploys `bytecode` (creation code) with optional ABI-encoded
    /// constructor arguments appended, from dev account `account`.
    pub fn deploy(
        &self,
        account: u64,
        bytecode: &str,
        constructor_args: &str,
    ) -> Result<Value, String> {
        let mut data = decode_hex(bytecode)?;
        data.extend(decode_hex(constructor_args)?);

        self.transact(account, TxKind::Create, U256::ZERO, data, DEPLOY_GAS)
    }

    /// Sends a transaction from dev account `account`.
    pub fn send(
        &self,
        account: u64,
        to: &str,
        data: &str,
        value_wei: &str,
    ) -> Result<Value, String> {
        let to: Address = to
            .trim()
            .parse()
            .map_err(|error| format!("invalid address: {error}"))?;

        let value = if value_wei.trim().is_empty() {
            U256::ZERO
        } else {
            U256::from_str_radix(value_wei.trim(), 10)
                .map_err(|error| format!("invalid value (decimal wei expected): {error}"))?
        };

        self.transact(
            account,
            TxKind::Call(to),
            value,
            decode_hex(data)?,
            SEND_GAS,
        )
    }

    /// Read-only call (`eth_call`) against the latest state. Returns the raw
    /// return data as hex.
    pub fn read(&self, to: &str, data: &str) -> Result<Value, String> {
        let result = self.json_rpc(
            "eth_call",
            json!([{ "to": to.trim(), "data": prefixed(data) }, "latest"]),
        )?;

        Ok(json!({ "result": result }))
    }

    /// ABI-encodes a call to `function` with `args` (a JSON array of strings or
    /// numbers, one per input) for the contract described by `abi` (JSON).
    /// Returns `{"data": "0x..."}`.
    pub fn encode(&self, abi: &str, function: &str, args: &str) -> Result<Value, String> {
        let function = find_function(abi, function, args)?;
        let values = parse_arguments(&function, args)?;

        let data = function
            .abi_encode_input(&values)
            .map_err(|error| format!("cannot encode call: {error}"))?;

        Ok(json!({ "data": format!("0x{}", hex::encode(data)) }))
    }

    /// Decodes the return data of `function` into JSON values.
    /// Returns `{"values": ["...", ...]}` (numbers as decimal strings).
    pub fn decode(&self, abi: &str, function: &str, data: &str) -> Result<Value, String> {
        let function = find_function(abi, function, "[]")?;

        let values = function
            .abi_decode_output(&decode_hex(data)?)
            .map_err(|error| format!("cannot decode result: {error}"))?;

        Ok(json!({ "values": values.iter().map(to_json).collect::<Vec<_>>() }))
    }

    fn transact(
        &self,
        account: u64,
        to: TxKind,
        value: U256,
        data: Vec<u8>,
        gas_limit: u64,
    ) -> Result<Value, String> {
        let node = self.node()?;

        let dev = DEV_ACCOUNTS
            .get(usize::try_from(account).map_err(|_| "account out of range")?)
            .ok_or_else(|| {
                format!(
                    "no dev account {account} (there are {})",
                    DEV_ACCOUNTS.len()
                )
            })?;

        let signer: PrivateKeySigner = dev
            .private_key
            .parse()
            .map_err(|error| format!("bad dev key: {error}"))?;

        let nonce = hex_u64(
            self.json_rpc(
                "eth_getTransactionCount",
                json!([dev.address.to_string(), "pending"]),
            )?
            .as_str()
            .ok_or("nonce is not a string")?,
        )?;

        let tx = TxEip1559 {
            chain_id: node.info().chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas: 0,
            max_priority_fee_per_gas: 0,
            to,
            value,
            access_list: Default::default(),
            input: Bytes::from(data),
        };

        let signature = signer
            .sign_hash_sync(&tx.signature_hash())
            .map_err(|error| format!("signing failed: {error}"))?;
        let envelope: TxEnvelope = tx.into_signed(signature).into();
        let raw = format!("0x{}", hex::encode(envelope.encoded_2718()));

        let hash = self
            .json_rpc("eth_sendRawTransaction", json!([raw]))?
            .as_str()
            .ok_or("transaction hash is not a string")?
            .to_string();

        self.wait_for_receipt(&hash)
    }

    fn wait_for_receipt(&self, hash: &str) -> Result<Value, String> {
        let deadline = Instant::now() + RECEIPT_TIMEOUT;

        loop {
            let receipt = self.json_rpc("eth_getTransactionReceipt", json!([hash]))?;

            if !receipt.is_null() {
                return Ok(json!({
                    "txHash": hash,
                    "status": receipt["status"],
                    "success": receipt["status"] == "0x1",
                    "contractAddress": receipt["contractAddress"],
                    "blockNumber": receipt["blockNumber"],
                    "gasUsed": receipt["gasUsed"],
                    "logs": receipt["logs"],
                }));
            }

            if Instant::now() >= deadline {
                return Err(format!(
                    "transaction {hash} was not mined within {RECEIPT_TIMEOUT:?}"
                ));
            }

            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn json_rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let response: Value = serde_json::from_str(&self.node()?.request(&request.to_string()))
            .map_err(|error| format!("unreadable RPC response: {error}"))?;

        if let Some(error) = response.get("error") {
            return Err(error["message"]
                .as_str()
                .unwrap_or("unknown RPC error")
                .to_string());
        }

        Ok(response["result"].clone())
    }
}

fn find_function(abi: &str, name: &str, args: &str) -> Result<alloy_json_abi::Function, String> {
    let abi =
        serde_json::from_str::<JsonAbi>(abi).map_err(|error| format!("invalid ABI: {error}"))?;

    let candidates = abi
        .function(name)
        .ok_or_else(|| format!("the ABI has no function {name:?}"))?;

    // Overloads: pick the one that takes this many arguments.
    let count = serde_json::from_str::<Vec<Value>>(args)
        .map(|list| list.len())
        .unwrap_or(0);

    candidates
        .iter()
        .find(|function| function.inputs.len() == count)
        .or_else(|| candidates.first())
        .cloned()
        .ok_or_else(|| format!("the ABI has no function {name:?}"))
}

fn parse_arguments(
    function: &alloy_json_abi::Function,
    args: &str,
) -> Result<Vec<DynSolValue>, String> {
    let list: Vec<Value> = if args.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(args)
            .map_err(|error| format!("arguments must be a JSON array: {error}"))?
    };

    if list.len() != function.inputs.len() {
        return Err(format!(
            "{} takes {} argument(s), got {}",
            function.name,
            function.inputs.len(),
            list.len()
        ));
    }

    function
        .inputs
        .iter()
        .zip(list)
        .map(|(input, value)| {
            let kind = Specifier::<alloy_dyn_abi::DynSolType>::resolve(input)
                .map_err(|error| format!("unsupported type {}: {error}", input.ty))?;

            // Numbers and booleans arrive as JSON values; compound types as JSON text.
            let text = match &value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };

            kind.coerce_str(&text).map_err(|error| {
                format!(
                    "argument {:?} is not a valid {}: {error}",
                    input.name, input.ty
                )
            })
        })
        .collect()
}

/// A decoded value as JSON: numbers as decimal strings, bytes as hex.
fn to_json(value: &DynSolValue) -> Value {
    match value {
        DynSolValue::Bool(flag) => json!(flag),
        DynSolValue::Int(number, _) => json!(number.to_string()),
        DynSolValue::Uint(number, _) => json!(number.to_string()),
        DynSolValue::Address(address) => json!(address.to_string()),
        DynSolValue::String(text) => json!(text),
        DynSolValue::FixedBytes(word, size) => json!(format!("0x{}", hex::encode(&word[..*size]))),
        DynSolValue::Bytes(bytes) => json!(format!("0x{}", hex::encode(bytes))),
        DynSolValue::Array(items) | DynSolValue::FixedArray(items) | DynSolValue::Tuple(items) => {
            Value::Array(items.iter().map(to_json).collect())
        }
        other => json!(format!("{other:?}")),
    }
}

fn describe(node: &EmbeddedNode) -> Value {
    json!({
        "running": true,
        "chainId": node.info().chain_id,
        "fresh": node.info().fresh,
        "head": node.head_height(),
        "halted": node.halted(),
        "publishing": node.info().publishing,
    })
}

fn prefixed(text: &str) -> String {
    let text = text.trim();

    if text.starts_with("0x") {
        text.to_string()
    } else {
        format!("0x{text}")
    }
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    let text = text.trim();
    let text = text.strip_prefix("0x").unwrap_or(text);

    hex::decode(text).map_err(|error| format!("invalid hex: {error}"))
}

fn hex_u64(text: &str) -> Result<u64, String> {
    u64::from_str_radix(text.trim_start_matches("0x"), 16)
        .map_err(|error| format!("invalid number {text:?}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const COUNTER_INITCODE: &str =
        include_str!("../vendor/solizone-evm/contracts/Counter.creation.hex");

    // Counter's selectors: increment() and count().
    const INCREMENT: &str = "0xd09de08a";
    const COUNT: &str = "0x06661abd";

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "solizone-module-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn nothing_works_before_start_and_errors_are_clear() {
        let engine = Engine::default();

        assert_eq!(engine.status()["running"], false);
        assert!(engine
            .deploy(0, "0x00", "")
            .unwrap_err()
            .contains("not running"));
        assert!(engine.rpc("{}").contains("not running"));
        assert_eq!(
            engine.accounts()["accounts"].as_array().unwrap().len(),
            DEV_ACCOUNTS.len()
        );
    }

    #[test]
    fn deploys_counter_increments_it_and_reads_it_back() {
        let dir = temp_dir("counter");
        let mut engine = Engine::default();

        let started = engine.start(dir.to_str().unwrap()).unwrap();
        assert_eq!(started["chainId"], 9001);
        assert_eq!(started["fresh"], true);

        let deployed = engine.deploy(0, COUNTER_INITCODE.trim(), "").unwrap();
        assert_eq!(deployed["success"], true);
        let address = deployed["contractAddress"].as_str().unwrap().to_string();

        for expected in 1..=2u64 {
            let sent = engine.send(0, &address, INCREMENT, "").unwrap();
            assert_eq!(sent["success"], true);

            let value = engine.read(&address, COUNT).unwrap();
            assert_eq!(
                hex_u64(value["result"].as_str().unwrap()).unwrap(),
                expected
            );
        }

        // Two accounts can transact at once without nonce clashes.
        let from_second = engine.send(1, &address, INCREMENT, "").unwrap();
        assert_eq!(from_second["success"], true);

        assert!(engine.status()["head"].as_u64().unwrap() >= 3);

        assert_eq!(engine.stop()["stopped"], true);
        assert_eq!(engine.stop()["stopped"], false);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failing_call_reports_failure_not_a_crash() {
        let dir = temp_dir("fail");
        let mut engine = Engine::default();
        engine.start(dir.to_str().unwrap()).unwrap();

        let deployed = engine.deploy(0, COUNTER_INITCODE.trim(), "").unwrap();
        let address = deployed["contractAddress"].as_str().unwrap().to_string();

        // fail() reverts on purpose.
        let reverted = engine.send(0, &address, "0xa9cc4718", "").unwrap();
        assert_eq!(reverted["success"], false);

        assert!(engine
            .send(99, &address, INCREMENT, "")
            .unwrap_err()
            .contains("no dev account"));
        assert!(engine.send(0, "not-an-address", INCREMENT, "").is_err());
        assert!(engine
            .send(0, &address, "0xzz", "")
            .unwrap_err()
            .contains("invalid hex"));

        engine.stop();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_chain_is_recovered_on_the_next_start() {
        let dir = temp_dir("recover");

        let mut first = Engine::default();
        first.start(dir.to_str().unwrap()).unwrap();
        let deployed = first.deploy(0, COUNTER_INITCODE.trim(), "").unwrap();
        let address = deployed["contractAddress"].as_str().unwrap().to_string();
        first.stop();

        let mut second = Engine::default();
        let started = second.start(dir.to_str().unwrap()).unwrap();
        assert_eq!(started["fresh"], false);

        let sent = second.send(0, &address, INCREMENT, "").unwrap();
        assert_eq!(sent["success"], true);

        second.stop();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const BOX_ABI: &str = r#"[
        {"type":"function","name":"set","stateMutability":"nonpayable",
         "inputs":[{"name":"v","type":"uint256"}],"outputs":[]},
        {"type":"function","name":"value","stateMutability":"view",
         "inputs":[],"outputs":[{"name":"","type":"uint256"}]},
        {"type":"function","name":"pair","stateMutability":"view",
         "inputs":[],"outputs":[{"name":"a","type":"address"},{"name":"ok","type":"bool"},{"name":"s","type":"string"}]}
    ]"#;

    #[test]
    fn encodes_calls_from_an_abi() {
        let engine = Engine::default();

        // set(uint256) has the selector 0x60fe47b1; the argument may be a string or a number.
        for args in [r#"["42"]"#, "[42]"] {
            let data = engine.encode(BOX_ABI, "set", args).unwrap()["data"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(data.starts_with("0x60fe47b1"), "{data}");
            assert!(data.ends_with("2a"), "{data}");
            assert_eq!(data.len(), 2 + 8 + 64);
        }

        assert_eq!(
            engine.encode(BOX_ABI, "value", "[]").unwrap()["data"],
            "0x3fa4f245"
        );
        assert_eq!(
            engine.encode(BOX_ABI, "value", "").unwrap()["data"],
            "0x3fa4f245"
        );
    }

    #[test]
    fn encoding_errors_are_readable() {
        let engine = Engine::default();

        assert!(engine
            .encode(BOX_ABI, "nope", "[]")
            .unwrap_err()
            .contains("no function"));
        assert!(engine
            .encode(BOX_ABI, "set", "[]")
            .unwrap_err()
            .contains("takes 1 argument"));
        assert!(engine
            .encode(BOX_ABI, "set", r#"["abc"]"#)
            .unwrap_err()
            .contains("not a valid uint256"));
        assert!(engine
            .encode("not json", "set", "[]")
            .unwrap_err()
            .contains("invalid ABI"));
        assert!(engine
            .encode(BOX_ABI, "set", "{")
            .unwrap_err()
            .contains("JSON array"));
    }

    #[test]
    fn decodes_return_values() {
        let engine = Engine::default();

        let word = format!("0x{:064x}", 42u64);
        assert_eq!(
            engine.decode(BOX_ABI, "value", &word).unwrap()["values"][0],
            "42"
        );

        // (address, bool, string) as the EVM returns them.
        let data = "0x\
            000000000000000000000000f39fd6e51aad88f6f4ce6ab8827279cfffb92266\
            0000000000000000000000000000000000000000000000000000000000000001\
            0000000000000000000000000000000000000000000000000000000000000060\
            0000000000000000000000000000000000000000000000000000000000000002\
            6869000000000000000000000000000000000000000000000000000000000000";
        let values = &engine.decode(BOX_ABI, "pair", data).unwrap()["values"];

        assert_eq!(values[0], "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        assert_eq!(values[1], true);
        assert_eq!(values[2], "hi");

        assert!(engine.decode(BOX_ABI, "value", "0x01").is_err());
    }

    #[test]
    fn start_is_idempotent_and_rejects_an_empty_directory() {
        let mut engine = Engine::default();
        assert!(engine.start("  ").is_err());

        let dir = temp_dir("idem");
        engine.start(dir.to_str().unwrap()).unwrap();
        assert_eq!(
            engine.start(dir.to_str().unwrap()).unwrap()["running"],
            true
        );

        engine.stop();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// Where the chain lives when the caller does not say: `~/.solicamp`.
pub fn default_data_dir() -> Result<String, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set; pass a data directory to start")?;

    Ok(PathBuf::from(home)
        .join(".solicamp")
        .to_string_lossy()
        .into_owned())
}
