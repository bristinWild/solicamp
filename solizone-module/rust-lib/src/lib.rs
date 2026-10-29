//! Solizone EVM as a Logos core module.
//!
//! A Solizone node runs inside the module (no child process, no open port).
//! Other modules and the SoliCamp UI call the methods below; every method
//! returns a JSON string, `{"error": "..."}` on failure.
//!
//! The trait IS the contract: the builder derives the `.lidl` from it
//! (`codegen.rust.trait` in metadata.json) and generates the C-ABI scaffold
//! around it. The logic lives in `engine`, which is plain Rust.

mod collab;
mod engine;
mod net;
mod publish;
mod rooms;
mod solc;
mod workspace;

/// Methods of the Solizone module. `account` is the index of a pre-funded dev
/// account (see `accounts`); amounts are decimal wei strings; bytecode, calldata
/// and constructor arguments are hex strings.
pub trait SolizoneModule: Send + 'static {
    /// Starts the chain in `data_dir`, or recovers the one stored there.
    fn start(&mut self, data_dir: String) -> String;

    /// Stops the chain. Its data stays on disk.
    fn stop(&mut self) -> String;

    /// Every room (separate local chain), `main` first: `{"rooms": [...]}`.
    fn list_rooms(&mut self) -> String;

    /// Creates a room, a new chain with its own id; it is not opened.
    fn create_room(&mut self, name: String) -> String;

    /// Switches to a room (stops the running chain, starts that one).
    fn open_room(&mut self, id: String) -> String;

    /// Makes a room shareable: `{"invite": "solicamp1:...", "topic": "..."}`.
    fn share_room(&mut self, id: String) -> String;

    /// Joins a room from an invite and opens it as a guest.
    fn join_room(&mut self, invite: String) -> String;

    /// Guest: `{"payload": hex}`, the message that asks the host for blocks.
    fn collab_hello(&mut self) -> String;

    /// Host: `{"payloads": [hex...]}`, blocks produced since the last call.
    fn collab_poll(&mut self) -> String;

    /// Handles a message (hex) received on the room topic:
    /// `{"send": [hex...], "applied": height|null, "note": text|null}`.
    fn collab_incoming(&mut self, payload: String) -> String;

    /// Goes online: starts the Logos Messaging node (once) and carries the
    /// open shared room's messages for you, now and after room switches.
    fn net_start(&mut self) -> String;

    /// Goes offline. Rooms and chains are untouched.
    fn net_stop(&mut self) -> String;

    /// `{"online", "phase", "connection", "topic", "error", "note", "sent",
    /// "received", "applied"}`; `online` is false before `net_start`.
    fn net_status(&mut self) -> String;

    /// Publishing state of the open room, as JSON: `state` is one of `off`,
    /// `guest`, `needs_node`, `needs_funding`, `syncing`, `published`,
    /// `error`, plus `message`, `channelId`, `walletAddress`, `balance`,
    /// `head`, `nextHeight`. Call it regularly: it also starts the publisher
    /// once the wallet is funded.
    fn publish_status(&mut self) -> String;

    /// Turns "Publish to Bedrock" on for the open room (nothing is published
    /// or paid for before this) and returns the status.
    fn publish_start(&mut self) -> String;

    /// Turns publishing off for the open room and returns the status.
    fn publish_stop(&mut self) -> String;

    /// Pays publishing fees from this wallet address (64 hex characters, one of
    /// the Basecamp wallet's addresses); an empty string goes back to the
    /// automatic choice. Returns the status.
    fn publish_use_address(&mut self, address: String) -> String;

    /// Uses an existing channel id and sequencer key (64 hex characters
    /// each, or the path of a file holding them) for the open room. Publishing
    /// must be off.
    fn publish_import(&mut self, channel_id: String, sequencer_key: String) -> String;

    /// A transaction's receipt, or `{"pending": true}` until a block has it.
    fn receipt(&mut self, tx_hash: String) -> String;

    /// `{"running", "chainId", "head", "halted", ...}`.
    fn status(&mut self) -> String;

    /// The pre-funded dev accounts: `{"accounts": [{"index", "address"}]}`.
    fn accounts(&mut self) -> String;

    /// Compiles one Solidity source: `{"contracts": [{"name", "abi", "bytecode"}]}`.
    fn compile(&mut self, source: String) -> String;

    /// Compiles a saved file of the open room's contracts folder, with the
    /// files it imports: `{"contracts": [{"name", "file", "abi", "bytecode"}]}`.
    fn compile_file(&mut self, path: String) -> String;

    /// The open room's contract files and folders, parents first:
    /// `{"entries": [{"path", "kind": "dir"|"file"}]}`. Paths are relative to
    /// the room's contracts folder.
    fn fs_list(&mut self) -> String;

    /// `{"path", "content"}` of a `.sol` file.
    fn fs_read(&mut self, path: String) -> String;

    /// Saves a `.sol` file, creating folders as needed; replaces the file.
    fn fs_write(&mut self, path: String, content: String) -> String;

    /// Creates a `.sol` file; fails if it already exists.
    fn fs_create(&mut self, path: String, content: String) -> String;

    /// Creates a folder.
    fn fs_mkdir(&mut self, path: String) -> String;

    /// Renames or moves a file or folder.
    fn fs_rename(&mut self, from: String, to: String) -> String;

    /// Deletes a file, or a folder with everything in it.
    fn fs_delete(&mut self, path: String) -> String;

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
    /// Shared with the network thread, which locks it only for short calls.
    engine: std::sync::Arc<std::sync::Mutex<engine::Engine>>,
    bridge: Option<net::Bridge>,
}

impl Solizone {
    fn engine(&self) -> std::sync::MutexGuard<'_, engine::Engine> {
        self.engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The Zone SDK's connection to Basecamp's blockchain_module. The SDK copy the
/// adapter is built on announces a declared module name as the origin of its
/// calls; declare ours before it builds its first client.
fn blockchain_adapter() -> logos_blockchain_zone_sdk_module_backend::NodeModuleClient {
    logos_rust_sdk_compat::set_module_origin("solizone_module");

    logos_blockchain_zone_sdk_module_backend::NodeModuleClient::new()
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

        let result = dir.and_then(|dir| self.engine().start(&dir));

        #[cfg(not(test))]
        if let Ok(status) = &result {
            emit_chain_started(&status.to_string());
        }

        json_or_error(result)
    }

    fn stop(&mut self) -> String {
        self.engine().stop().to_string()
    }

    fn list_rooms(&mut self) -> String {
        json_or_error(self.engine().list_rooms())
    }

    fn create_room(&mut self, name: String) -> String {
        json_or_error(self.engine().create_room(&name))
    }

    fn open_room(&mut self, id: String) -> String {
        json_or_error(self.engine().open_room(&id))
    }

    fn share_room(&mut self, id: String) -> String {
        json_or_error(self.engine().share_room(&id))
    }

    fn join_room(&mut self, invite: String) -> String {
        json_or_error(self.engine().join_room(&invite))
    }

    fn collab_hello(&mut self) -> String {
        json_or_error(self.engine().collab_hello())
    }

    fn collab_poll(&mut self) -> String {
        json_or_error(self.engine().collab_poll())
    }

    fn collab_incoming(&mut self, payload: String) -> String {
        json_or_error(self.engine().collab_incoming(&payload))
    }

    fn net_start(&mut self) -> String {
        if self.bridge.is_none() {
            let (network, incoming) = net::LogosNet::new();

            self.bridge = Some(net::Bridge::start(
                self.engine.clone(),
                network,
                incoming,
                net::Timing::default(),
            ));
        }

        self.net_status()
    }

    fn net_stop(&mut self) -> String {
        let was_online = self.bridge.is_some();

        if let Some(bridge) = self.bridge.take() {
            bridge.stop();
        }

        serde_json::json!({ "online": false, "wasOnline": was_online }).to_string()
    }

    fn net_status(&mut self) -> String {
        match &self.bridge {
            Some(bridge) => {
                let mut status = bridge.status();
                status["online"] = serde_json::Value::Bool(true);
                status.to_string()
            }
            None => serde_json::json!({ "online": false }).to_string(),
        }
    }

    fn publish_status(&mut self) -> String {
        json_or_error(
            self.engine()
                .publish_status(&publish::BasecampWallet, blockchain_adapter),
        )
    }

    fn publish_start(&mut self) -> String {
        json_or_error(
            self.engine()
                .publish_set(true, &publish::BasecampWallet, blockchain_adapter),
        )
    }

    fn publish_stop(&mut self) -> String {
        json_or_error(self.engine().publish_set(
            false,
            &publish::BasecampWallet,
            blockchain_adapter,
        ))
    }

    fn publish_use_address(&mut self, address: String) -> String {
        json_or_error(self.engine().publish_use_address(
            &address,
            &publish::BasecampWallet,
            blockchain_adapter,
        ))
    }

    fn publish_import(&mut self, channel_id: String, sequencer_key: String) -> String {
        json_or_error(self.engine().publish_import(&channel_id, &sequencer_key))
    }

    fn receipt(&mut self, tx_hash: String) -> String {
        json_or_error(self.engine().receipt(&tx_hash))
    }

    fn status(&mut self) -> String {
        self.engine().status().to_string()
    }

    fn accounts(&mut self) -> String {
        self.engine().accounts().to_string()
    }

    fn compile(&mut self, source: String) -> String {
        json_or_error(self.engine().compile(&source))
    }

    fn compile_file(&mut self, path: String) -> String {
        json_or_error(self.engine().compile_file(&path))
    }

    fn fs_list(&mut self) -> String {
        json_or_error(self.engine().fs_list())
    }

    fn fs_read(&mut self, path: String) -> String {
        json_or_error(self.engine().fs_read(&path))
    }

    fn fs_write(&mut self, path: String, content: String) -> String {
        json_or_error(self.engine().fs_write(&path, &content))
    }

    fn fs_create(&mut self, path: String, content: String) -> String {
        json_or_error(self.engine().fs_create(&path, &content))
    }

    fn fs_mkdir(&mut self, path: String) -> String {
        json_or_error(self.engine().fs_mkdir(&path))
    }

    fn fs_rename(&mut self, from: String, to: String) -> String {
        json_or_error(self.engine().fs_rename(&from, &to))
    }

    fn fs_delete(&mut self, path: String) -> String {
        json_or_error(self.engine().fs_delete(&path))
    }

    fn deploy(&mut self, account: u64, bytecode: String, constructor_args: String) -> String {
        json_or_error(self.engine().deploy(account, &bytecode, &constructor_args))
    }

    fn send(&mut self, account: u64, to: String, data: String, value_wei: String) -> String {
        json_or_error(self.engine().send(account, &to, &data, &value_wei))
    }

    fn encode(&mut self, abi: String, function: String, args: String) -> String {
        json_or_error(self.engine().encode(&abi, &function, &args))
    }

    fn decode(&mut self, abi: String, function: String, data: String) -> String {
        json_or_error(self.engine().decode(&abi, &function, &data))
    }

    fn read(&mut self, to: String, data: String) -> String {
        json_or_error(self.engine().read(&to, &data))
    }

    fn rpc(&mut self, request: String) -> String {
        self.engine().rpc(&request)
    }
}

#[cfg(not(test))]
#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<Solizone>();
}
