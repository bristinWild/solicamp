//! Solizone EVM as a Logos core module.
//!
//! A Solizone node runs inside the module (no child process, no open port).
//! Other modules and the SoliCamp UI call the methods below; every method
//! returns a JSON string, `{"error": "..."}` on failure.
//!
//! The trait IS the contract: the builder derives the `.lidl` from it
//! (`codegen.rust.trait` in metadata.json) and generates the C-ABI scaffold
//! around it. The logic lives in `engine`, which is plain Rust.

mod engine;
mod solc;

/// Methods of the Solizone module. `account` is the index of a pre-funded dev
/// account (see `accounts`); amounts are decimal wei strings; bytecode, calldata
/// and constructor arguments are hex strings.
pub trait SolizoneModule: Send + 'static {
    /// Starts the chain in `data_dir`, or recovers the one stored there.
    fn start(&mut self, data_dir: String) -> String;

    /// Stops the chain. Its data stays on disk.
    fn stop(&mut self) -> String;

    /// `{"running", "chainId", "head", "halted", ...}`.
    fn status(&mut self) -> String;

    /// The pre-funded dev accounts: `{"accounts": [{"index", "address"}]}`.
    fn accounts(&mut self) -> String;

    /// Compiles one Solidity source: `{"contracts": [{"name", "abi", "bytecode"}]}`.
    fn compile(&mut self, source: String) -> String;

    /// Deploys creation `bytecode` (with ABI-encoded `constructor_args`
    /// appended) and waits for the receipt.
    fn deploy(&mut self, account: u64, bytecode: String, constructor_args: String) -> String;

    /// Sends a transaction and waits for the receipt.
    fn send(&mut self, account: u64, to: String, data: String, value_wei: String) -> String;

    /// ABI-encodes a call: `abi` is the contract's ABI (JSON), `args` a JSON
    /// array with one entry per input. Returns `{"data": "0x..."}`.
    fn encode(&mut self, abi: String, function: String, args: String) -> String;

    /// Decodes a call's return data into `{"values": [...]}`.
    fn decode(&mut self, abi: String, function: String, data: String) -> String;

    /// Read-only call against the latest state: `{"result": "0x..."}`.
    fn read(&mut self, to: String, data: String) -> String;

    /// One raw JSON-RPC request in, one response out. The same methods as the
    /// HTTP endpoint (`eth_*`, `net_*`, `web3_*`, `solizone_*`).
    fn rpc(&mut self, request: String) -> String;

    /// Called once the module is wired to the host.
    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}
}

/// Typed events other modules can subscribe to.
pub trait SolizoneModuleEvents {
    /// The chain started (or was recovered); the payload is the `status` JSON.
    fn chain_started(&self, status: String);
}

// The builder injects the generated scaffold here: `install`,
// `RustModuleContext`, `context()`, `modules()` and the `emit_*` emitters.
#[cfg(not(test))]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/generated/provider_gen.rs"
));

// `cargo test` has no generated scaffold; the context type is only named by
// the trait's default hook.
#[cfg(test)]
pub struct RustModuleContext;

#[derive(Default)]
struct Solizone {
    engine: engine::Engine,
}

fn json_or_error(result: Result<serde_json::Value, String>) -> String {
    match result {
        Ok(value) => value.to_string(),
        Err(message) => serde_json::json!({ "error": message }).to_string(),
    }
}

impl SolizoneModule for Solizone {
    fn start(&mut self, data_dir: String) -> String {
        let dir = if data_dir.trim().is_empty() {
            engine::default_data_dir()
        } else {
            Ok(data_dir)
        };

        let result = dir.and_then(|dir| self.engine.start(&dir));

        #[cfg(not(test))]
        if let Ok(status) = &result {
            emit_chain_started(&status.to_string());
        }

        json_or_error(result)
    }

    fn stop(&mut self) -> String {
        self.engine.stop().to_string()
    }

    fn status(&mut self) -> String {
        self.engine.status().to_string()
    }

    fn accounts(&mut self) -> String {
        self.engine.accounts().to_string()
    }

    fn compile(&mut self, source: String) -> String {
        json_or_error(self.engine.compile(&source))
    }

    fn deploy(&mut self, account: u64, bytecode: String, constructor_args: String) -> String {
        json_or_error(self.engine.deploy(account, &bytecode, &constructor_args))
    }

    fn send(&mut self, account: u64, to: String, data: String, value_wei: String) -> String {
        json_or_error(self.engine.send(account, &to, &data, &value_wei))
    }

    fn encode(&mut self, abi: String, function: String, args: String) -> String {
        json_or_error(self.engine.encode(&abi, &function, &args))
    }

    fn decode(&mut self, abi: String, function: String, data: String) -> String {
        json_or_error(self.engine.decode(&abi, &function, &data))
    }

    fn read(&mut self, to: String, data: String) -> String {
        json_or_error(self.engine.read(&to, &data))
    }

    fn rpc(&mut self, request: String) -> String {
        self.engine.rpc(&request)
    }
}

#[cfg(not(test))]
#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<Solizone>();
}
