//! The module's logic, free of any Logos plumbing so it can be unit-tested
//! with plain `cargo test`.
//!
//! Everything returns JSON values or `Err(message)`; the module layer turns
//! those into JSON strings (`{"error": "..."}` on failure).

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_dyn_abi::{DynSolValue, FunctionExt, JsonAbiExt, Specifier};
use alloy_eips::eip2718::Encodable2718;
use alloy_json_abi::JsonAbi;
use alloy_primitives::{keccak256, Address, Bytes, TxKind, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use serde_json::{json, Value};
use solizone_evm::{
    embedded::{EmbeddedConfig, EmbeddedNode},
    node::dev_accounts::DEV_ACCOUNTS,
};

use crate::{
    collab::{self, Invite, Session},
    publish::{self, PublishDir, Wallet},
    rooms::{CollabInfo, Role, Room, Rooms, MAIN_ID},
    solc,
};

const DEPLOY_GAS: u64 = 3_000_000;
const SEND_GAS: u64 = 1_000_000;
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(20);
/// Most unpublished guest messages kept while the network is down.
const MAX_OUTBOX: usize = 256;

#[derive(Default)]
pub struct Engine {
    node: Option<EmbeddedNode>,
    /// The room the running node belongs to.
    room: Option<Room>,
    /// Base directory holding the rooms (set by the first start).
    base: Option<PathBuf>,
    solc: Option<PathBuf>,
    /// Collaboration state of the open room, if it is shared.
    session: Option<Session>,
    /// Guests only: the next nonce per dev account, counting transactions
    /// sent to the host that no block contains yet.
    guest_nonces: HashMap<u64, u64>,
    /// Guests only: messages for the host (signed transactions) that the
    /// network bridge has not published yet.
    outbox: Vec<Vec<u8>>,
}

impl Engine {
    fn node(&self) -> Result<&EmbeddedNode, String> {
        self.node
            .as_ref()
            .ok_or_else(|| "the chain is not running; call start first".to_string())
    }

    /// Starts the main room in `data_dir` (or recovers the one stored there).
    pub fn start(&mut self, data_dir: &str) -> Result<Value, String> {
        if let Some(node) = &self.node {
            return Ok(self.describe(node));
        }

        if data_dir.trim().is_empty() {
            return Err("start needs a data directory".to_string());
        }

        self.base = Some(PathBuf::from(data_dir));
        self.open_room_in(MAIN_ID)
    }

    fn rooms(&self) -> Result<Rooms, String> {
        match &self.base {
            Some(base) => Ok(Rooms::new(base)),
            None => Ok(Rooms::new(default_data_dir()?)),
        }
    }

    /// All rooms, `main` first.
    pub fn list_rooms(&self) -> Result<Value, String> {
        let rooms = self.rooms()?;
        let current = self.room.as_ref().map(|room| room.id.clone());

        let list: Vec<Value> = rooms
            .list()?
            .iter()
            .map(|room| {
                let mut entry = room.to_json();
                entry["open"] = json!(current.as_deref() == Some(room.id.as_str()));
                entry
            })
            .collect();

        Ok(json!({ "rooms": list }))
    }

    /// Makes a room shareable and returns its invite. The first call makes
    /// you the host (new room key and host signing key); later calls return
    /// the same invite. `main` cannot be shared.
    pub fn share_room(&mut self, id: &str) -> Result<Value, String> {
        if id == MAIN_ID {
            return Err(
                "Main is the canonical dev chain and cannot be shared; create a room".to_string(),
            );
        }

        let rooms = self.rooms()?;
        let room = rooms.get(id)?;

        let info = match rooms.collab(&room)? {
            Some(info) => info,
            None => {
                let signer = collab::new_host_key();
                let info = CollabInfo {
                    role: Role::Host,
                    key: collab::new_room_key(),
                    host: signer.address(),
                    host_key: Some(collab::signer_to_hex(&signer)),
                };

                rooms.set_collab(&room, &info)?;
                info
            }
        };

        // The room may already be open without a session: start hosting now.
        let is_open = self.room.as_ref().is_some_and(|open| open.id == room.id);

        if is_open && self.session.is_none() {
            if let Some(node) = &self.node {
                self.session = Some(session_for(&room, &info, node)?);
            }
        }

        let invite = invite_for(&room, &info);

        Ok(json!({
            "invite": invite.encode(),
            "topic": invite.topic(),
            "role": info.role.as_str(),
        }))
    }

    /// Joins a room from an invite and opens it as a follower.
    pub fn join_room(&mut self, invite: &str) -> Result<Value, String> {
        let invite = Invite::decode(invite)?;

        if self.base.is_none() {
            self.base = Some(PathBuf::from(default_data_dir()?));
        }

        let rooms = self.rooms()?;
        let room = rooms.adopt(&invite.room_id, &invite.name, invite.chain_id)?;

        rooms.set_collab(
            &room,
            &CollabInfo {
                role: Role::Guest,
                key: invite.key,
                host: invite.host,
                host_key: None,
            },
        )?;

        self.open_room(&room.id)
    }

    fn collab_parts(&mut self) -> Result<(&EmbeddedNode, &mut Session), String> {
        let node = self
            .node
            .as_ref()
            .ok_or("the chain is not running; call start first")?;
        let session = self
            .session
            .as_mut()
            .ok_or("this room is not shared; use share_room or join_room")?;

        Ok((node, session))
    }

    /// Guest: the message that asks the host for the blocks we are missing.
    /// Send it when joining and every few seconds while behind.
    pub fn collab_hello(&mut self) -> Result<Value, String> {
        let (node, session) = self.collab_parts()?;

        if session.is_host() {
            return Err("only guests ask the host for blocks".to_string());
        }

        Ok(json!({ "payload": hex::encode(session.hello(node)?) }))
    }

    /// Host: messages carrying the blocks produced since the last call.
    pub fn collab_poll(&mut self) -> Result<Value, String> {
        let (node, session) = self.collab_parts()?;

        if !session.is_host() {
            return Err("only the host publishes blocks".to_string());
        }

        let payloads: Vec<String> = session.poll(node)?.iter().map(hex::encode).collect();

        Ok(json!({ "payloads": payloads }))
    }

    /// Handles one message received on the room topic (hex). Returns the
    /// messages to publish in answer, the height a guest just applied and a
    /// note for the user when something was refused.
    pub fn collab_incoming(&mut self, payload: &str) -> Result<Value, String> {
        let bytes = decode_hex(payload)?;
        let (node, session) = self.collab_parts()?;

        let outcome = session.handle(node, &bytes);

        Ok(json!({
            "send": outcome.send.iter().map(hex::encode).collect::<Vec<_>>(),
            "applied": outcome.applied,
            "note": outcome.note,
        }))
    }

    /// The shared room the network should serve right now, if any:
    /// `(content topic, is_host)`.
    pub fn collab_room(&self) -> Option<(String, bool)> {
        self.node.as_ref()?;
        let session = self.session.as_ref()?;

        Some((session.invite().topic(), session.is_host()))
    }

    /// Guest: the messages for the host that are waiting to be published
    /// (signed transactions), oldest first. Emptied by the call.
    pub fn take_outbox(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.outbox)
    }

    /// Creates a room (a fresh chain with its own id). It is not opened.
    pub fn create_room(&mut self, name: &str) -> Result<Value, String> {
        Ok(self.rooms()?.create(name)?.to_json())
    }

    /// Switches to another room: stops the running chain and starts that one.
    pub fn open_room(&mut self, id: &str) -> Result<Value, String> {
        if self.base.is_none() {
            self.base = Some(PathBuf::from(default_data_dir()?));
        }

        // Validate before touching the running node.
        self.rooms()?.get(id)?;

        self.node.take().map(EmbeddedNode::stop);
        self.room = None;
        self.session = None;

        self.open_room_in(id)
    }

    fn open_room_in(&mut self, id: &str) -> Result<Value, String> {
        let rooms = self.rooms()?;
        let room = rooms.get(id)?;
        let collab = rooms.collab(&room)?;

        let node = EmbeddedNode::start(EmbeddedConfig {
            chain_id: room.chain_id,
            follower: collab.as_ref().is_some_and(|info| info.role == Role::Guest),
            block_time: Duration::from_millis(500),
            ..EmbeddedConfig::local(rooms.data_dir(&room))
        })?;

        self.session = match &collab {
            Some(info) => Some(session_for(&room, info, &node)?),
            None => None,
        };
        self.guest_nonces.clear();
        self.outbox.clear();

        self.room = Some(room);
        let info = self.describe(&node);
        self.node = Some(node);

        Ok(info)
    }

    fn describe(&self, node: &EmbeddedNode) -> Value {
        let mut info = describe(node);

        if let Some(room) = &self.room {
            info["room"] = room.to_json();
        }

        if let Some(session) = &self.session {
            info["collab"] = json!({
                "role": if session.is_host() { "host" } else { "guest" },
                "topic": session.invite().topic(),
            });
        }

        info
    }

    // -- publishing ---------------------------------------------------------

    fn publish_dir(&self) -> Result<PublishDir, String> {
        let room = self.room.as_ref().ok_or("no room is open")?;

        Ok(PublishDir::new(&self.rooms()?.data_dir(room)))
    }

    /// Brings publishing in line with the room's setting and returns its
    /// status. Call it regularly (the UI polls): it starts the publisher
    /// once the wallet is funded, restarts it after a restart of the app,
    /// and stops it when the setting is off or the room is not ours to
    /// publish. `adapter` builds the Logos node connection and is only
    /// called when a publisher is actually started.
    pub fn publish_status<N>(
        &self,
        wallet: &dyn Wallet,
        adapter: impl FnOnce() -> N,
    ) -> Result<Value, String>
    where
        N: solizone_evm::publication::logos::LogosNode + Clone + Send + Sync + 'static,
    {
        let node = self.node()?;
        let dir = self.publish_dir()?;

        let enabled = dir.enabled();
        let is_host = self.session.as_ref().is_none_or(Session::is_host);
        let should_run = enabled && is_host;

        // The wallet has several keys; pay from the one that holds funds.
        let publish::Funding {
            address: wallet_address,
            balance,
        } = publish::choose_funding(wallet, dir.funding_address().as_deref());

        let mut running = node.publication_state().is_some();

        if running && !should_run {
            node.stop_publishing();
            running = false;
        }

        if should_run && !running {
            if let (Ok(Some(address)), Some(Ok(funds))) = (&wallet_address, &balance) {
                if *funds > 0 {
                    let keys = dir.keys()?;

                    dir.set_funding_address(address)?;

                    node.start_publishing(
                        solizone_evm::publication::logos::PublisherSettings {
                            channel_id: keys.channel_id,
                            sequencer_key: keys.sequencer_key,
                            funding_address: publish::parse_hex_32(address)?,
                            max_tx_fee: publish::MAX_TX_FEE,
                            feeder: solizone_evm::publication::FeederConfig::default(),
                        },
                        adapter(),
                    )?;

                    running = true;
                }
            }
        }

        let progress = node
            .publication_state()
            .or_else(|| node.saved_publication_state().ok());

        // Show the channel only once it exists; looking never creates keys.
        let keys = dir.keys_if_present();

        Ok(publish::status_json(
            &publish::Inputs {
                enabled,
                is_host,
                running,
                wallet_address,
                balance,
                head: node.head_height(),
                progress: progress.as_ref(),
            },
            keys.as_ref(),
        ))
    }

    /// Turns publishing on or off for the open room (persisted), then
    /// applies it.
    pub fn publish_set<N>(
        &self,
        enabled: bool,
        wallet: &dyn Wallet,
        adapter: impl FnOnce() -> N,
    ) -> Result<Value, String>
    where
        N: solizone_evm::publication::logos::LogosNode + Clone + Send + Sync + 'static,
    {
        let dir = self.publish_dir()?;

        if enabled
            && self
                .session
                .as_ref()
                .is_some_and(|session| !session.is_host())
        {
            return Err("only the host of a shared room can publish it".to_string());
        }

        if enabled {
            // Creates the room's channel and key on first use.
            dir.keys()?;
        }

        dir.set_enabled(enabled)?;

        self.publish_status(wallet, adapter)
    }

    /// Uses an existing channel id and sequencer key for the open room.
    pub fn publish_import(&self, channel: &str, key: &str) -> Result<Value, String> {
        let keys = self.publish_dir()?.import(channel, key)?;

        Ok(json!({ "channelId": hex::encode(keys.channel_id) }))
    }

    pub fn stop(&mut self) -> Value {
        let was_running = self.node.take().map(EmbeddedNode::stop).is_some();
        self.room = None;
        self.session = None;
        self.outbox.clear();

        json!({ "stopped": was_running })
    }

    pub fn status(&self) -> Value {
        match &self.node {
            Some(node) => self.describe(node),
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
        &mut self,
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
        &mut self,
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
        &mut self,
        account: u64,
        to: TxKind,
        value: U256,
        data: Vec<u8>,
        gas_limit: u64,
    ) -> Result<Value, String> {
        let guest = self
            .session
            .as_ref()
            .is_some_and(|session| !session.is_host());
        let chain_id = self.node()?.info().chain_id;

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

        let mut nonce = hex_u64(
            self.json_rpc(
                "eth_getTransactionCount",
                json!([dev.address.to_string(), "pending"]),
            )?
            .as_str()
            .ok_or("nonce is not a string")?,
        )?;

        if guest {
            // Transactions we sent that no block holds yet still count.
            nonce = nonce.max(self.guest_nonces.get(&account).copied().unwrap_or(0));
        }

        let tx = TxEip1559 {
            chain_id,
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
        let encoded = envelope.encoded_2718();

        if guest {
            // Hand the transaction to the host; the receipt appears once the
            // host's block with it arrives (poll `receipt`).
            self.guest_nonces.insert(account, nonce + 1);

            let payload = self
                .session
                .as_ref()
                .ok_or("not in a shared room")?
                .submit(&encoded)?;

            if self.outbox.len() < MAX_OUTBOX {
                self.outbox.push(payload.clone());
            }

            return Ok(json!({
                "pending": true,
                "txHash": keccak256(&encoded).to_string(),
                "payload": hex::encode(payload),
            }));
        }

        let raw = format!("0x{}", hex::encode(&encoded));

        let hash = self
            .json_rpc("eth_sendRawTransaction", json!([raw]))?
            .as_str()
            .ok_or("transaction hash is not a string")?
            .to_string();

        self.wait_for_receipt(&hash)
    }

    /// The receipt of a transaction, or `{"pending": true}` while no block
    /// holds it (in a shared room, until the host's block arrives).
    pub fn receipt(&self, hash: &str) -> Result<Value, String> {
        let receipt = self.json_rpc("eth_getTransactionReceipt", json!([hash.trim()]))?;

        Ok(if receipt.is_null() {
            json!({ "pending": true, "txHash": hash.trim() })
        } else {
            receipt_json(hash.trim(), &receipt)
        })
    }

    fn wait_for_receipt(&self, hash: &str) -> Result<Value, String> {
        let deadline = Instant::now() + RECEIPT_TIMEOUT;

        loop {
            let receipt = self.json_rpc("eth_getTransactionReceipt", json!([hash]))?;

            if !receipt.is_null() {
                return Ok(receipt_json(hash, &receipt));
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

fn receipt_json(hash: &str, receipt: &Value) -> Value {
    json!({
        "txHash": hash,
        "status": receipt["status"],
        "success": receipt["status"] == "0x1",
        "contractAddress": receipt["contractAddress"],
        "blockNumber": receipt["blockNumber"],
        "gasUsed": receipt["gasUsed"],
        "logs": receipt["logs"],
    })
}

fn invite_for(room: &Room, info: &CollabInfo) -> Invite {
    Invite {
        room_id: room.id.clone(),
        name: room.name.clone(),
        chain_id: room.chain_id,
        key: info.key,
        host: info.host,
    }
}

fn session_for(room: &Room, info: &CollabInfo, node: &EmbeddedNode) -> Result<Session, String> {
    let invite = invite_for(room, info);

    match info.role {
        Role::Host => {
            let signer = collab::signer_from_hex(
                info.host_key.as_deref().ok_or("the host key is missing")?,
            )?;

            Session::host(invite, signer, node)
        }
        Role::Guest => Ok(Session::guest(invite)),
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
        let mut engine = Engine::default();

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

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);

        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn payloads(value: &Value) -> Vec<String> {
        value["send"]
            .as_array()
            .or_else(|| value["payloads"].as_array())
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect()
    }

    /// Delivers hex payloads to `to`, and whatever it answers back to `from`,
    /// until the conversation ends.
    fn exchange(from: &mut Engine, to: &mut Engine, mut queue: Vec<String>) {
        let mut forward = true;

        while !queue.is_empty() {
            let receiver: &mut Engine = if forward { to } else { from };
            let mut answers = Vec::new();

            for payload in queue {
                answers.extend(payloads(&receiver.collab_incoming(&payload).unwrap()));
            }

            queue = answers;
            forward = !forward;
        }
    }

    #[test]
    fn two_engines_collaborate_in_a_shared_room() {
        let host_dir = temp_dir("collab-host");
        let guest_dir = temp_dir("collab-guest");

        let mut host = Engine::default();
        let mut guest = Engine::default();

        host.start(host_dir.to_str().unwrap()).unwrap();
        guest.start(guest_dir.to_str().unwrap()).unwrap();

        // Main cannot be shared; other rooms can.
        assert!(host
            .share_room("main")
            .unwrap_err()
            .contains("cannot be shared"));
        assert!(host.collab_poll().unwrap_err().contains("not shared"));

        let room = host.create_room("Team").unwrap();
        let id = room["id"].as_str().unwrap().to_string();
        host.open_room(&id).unwrap();

        let shared = host.share_room(&id).unwrap();
        let invite = shared["invite"].as_str().unwrap().to_string();
        assert_eq!(host.share_room(&id).unwrap()["invite"], invite.as_str());
        assert_eq!(host.status()["collab"]["role"], "host");

        // Role misuse is refused.
        assert!(host.collab_hello().unwrap_err().contains("only guests"));

        assert!(guest.join_room("junk").is_err());
        let joined = guest.join_room(&invite).unwrap();
        assert_eq!(joined["room"]["id"], id.as_str());
        assert_eq!(joined["chainId"], room["chainId"]);
        assert_eq!(joined["collab"]["role"], "guest");
        assert_eq!(joined["head"], Value::Null, "a guest starts with no chain");
        assert!(guest.join_room(&invite).unwrap_err().contains("already"));
        assert!(guest.collab_poll().unwrap_err().contains("only the host"));

        // Joining: ask for blocks, receive the host's genesis.
        let hello = guest.collab_hello().unwrap();
        exchange(
            &mut guest,
            &mut host,
            vec![hello["payload"].as_str().unwrap().to_string()],
        );
        assert_eq!(guest.status()["head"], 0);

        // The guest deploys: the transaction goes to the host, which orders it.
        let sent = guest.deploy(0, COUNTER_INITCODE.trim(), "").unwrap();
        assert_eq!(sent["pending"], true);
        let hash = sent["txHash"].as_str().unwrap().to_string();
        assert_eq!(guest.receipt(&hash).unwrap()["pending"], true);

        let payload = sent["payload"].as_str().unwrap().to_string();
        assert!(payloads(&host.collab_incoming(&payload).unwrap()).is_empty());

        wait_until("the host's block", || host.status()["head"] == 1);

        // The host publishes; the guest applies and now has the receipt.
        let blocks = payloads(&host.collab_poll().unwrap());
        assert_eq!(blocks.len(), 1);
        exchange(&mut host, &mut guest, blocks);

        let receipt = guest.receipt(&hash).unwrap();
        assert_eq!(receipt["success"], true);
        let address = receipt["contractAddress"].as_str().unwrap().to_string();

        // Both sides see the same contract, and the guest can drive it.
        for engine in [&mut host, &mut guest] {
            let code = engine.rpc(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"eth_getCode","params":["{address}","latest"]}}"#
            ));
            assert_ne!(code, r#"{"jsonrpc":"2.0","id":1,"result":"0x"}"#);
        }

        // Two quick sends from the same guest account get consecutive nonces.
        let first = guest.send(0, &address, INCREMENT, "").unwrap();
        let second = guest.send(0, &address, INCREMENT, "").unwrap();
        for sent in [&first, &second] {
            let payload = sent["payload"].as_str().unwrap().to_string();
            host.collab_incoming(&payload).unwrap();
        }

        wait_until("both increments", || {
            let value = host.read(&address, COUNT).unwrap();
            hex_u64(value["result"].as_str().unwrap()).unwrap() == 2
        });

        let blocks = payloads(&host.collab_poll().unwrap());
        exchange(&mut host, &mut guest, blocks);

        let seen_by_guest = guest.read(&address, COUNT).unwrap();
        assert_eq!(
            hex_u64(seen_by_guest["result"].as_str().unwrap()).unwrap(),
            2
        );
        assert_eq!(guest.status()["head"], host.status()["head"]);

        // Both survive a restart with their roles and chains intact.
        host.stop();
        guest.stop();

        host.start(host_dir.to_str().unwrap()).unwrap();
        guest.start(guest_dir.to_str().unwrap()).unwrap();

        let host_back = host.open_room(&id).unwrap();
        let guest_back = guest.open_room(&id).unwrap();
        assert_eq!(host_back["collab"]["role"], "host");
        assert_eq!(guest_back["collab"]["role"], "guest");
        assert_eq!(guest_back["head"], host_back["head"]);

        host.stop();
        guest.stop();
        std::fs::remove_dir_all(&host_dir).unwrap();
        std::fs::remove_dir_all(&guest_dir).unwrap();
    }

    #[test]
    fn rooms_are_separate_chains_that_keep_their_own_history() {
        let dir = temp_dir("rooms");
        let mut engine = Engine::default();

        let main = engine.start(dir.to_str().unwrap()).unwrap();
        assert_eq!(main["room"]["id"], "main");

        let deployed = engine.deploy(0, COUNTER_INITCODE.trim(), "").unwrap();
        let main_address = deployed["contractAddress"].as_str().unwrap().to_string();
        let main_head = engine.status()["head"].as_u64().unwrap();

        let room = engine.create_room("  Workshop ").unwrap();
        let id = room["id"].as_str().unwrap().to_string();
        let room_chain = room["chainId"].as_u64().unwrap();
        assert_ne!(room_chain, 9001);

        // Creating does not switch.
        assert_eq!(engine.status()["room"]["id"], "main");

        let opened = engine.open_room(&id).unwrap();
        assert_eq!(opened["chainId"], room_chain);
        assert_eq!(opened["fresh"], true);
        assert_eq!(opened["head"], 0);

        // The main room's contract does not exist here, and the new chain signs for its own id.
        assert_eq!(
            engine.rpc(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"eth_getCode","params":["{main_address}","latest"]}}"#
            )),
            r#"{"jsonrpc":"2.0","id":1,"result":"0x"}"#
        );
        let here = engine.deploy(0, COUNTER_INITCODE.trim(), "").unwrap();
        assert_eq!(here["success"], true);

        let list = engine.list_rooms().unwrap();
        let rooms = list["rooms"].as_array().unwrap();
        assert_eq!(rooms.len(), 2);
        assert_eq!(rooms[0]["id"], "main");
        assert_eq!(rooms[0]["open"], false);
        assert_eq!(rooms[1]["open"], true);

        // Back to main: its history is exactly as we left it.
        let back = engine.open_room("main").unwrap();
        assert_eq!(back["chainId"], 9001);
        assert_eq!(back["fresh"], false);
        assert_eq!(back["head"], main_head);

        assert!(engine.open_room("missing").unwrap_err().contains("no room"));
        // A failed switch leaves the chain running.
        assert_eq!(engine.status()["room"]["id"], "main");

        engine.stop();
        std::fs::remove_dir_all(&dir).unwrap();
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
