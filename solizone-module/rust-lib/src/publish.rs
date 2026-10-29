//! "Publish to Bedrock": per-room publishing settings, wallet access and the
//! status the UI shows.
//!
//! A local chain is a sandbox. Publishing is a separate, explicit step: it is
//! off until the user turns it on, and only then do inscriptions (and fees)
//! start. Each room publishes to its own Logos channel with its own sequencer
//! key, both generated on first use and kept in `<room dir>/publish/`; the
//! user can also import an existing channel id and key (for example the
//! canonical Solizone channel).
//!
//! Everything here is plain Rust so it is testable without Basecamp; the
//! real wallet and node adapter are supplied by the module layer.

use std::{
    fs,
    path::{Path, PathBuf},
};

use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Value};
use solizone_evm::publication::{PublicationState, PublicationStatus};

/// Files in `<room dir>/publish/`.
const CHANNEL_FILE: &str = "channel_id";
const KEY_FILE: &str = "sequencer_key";
const ENABLED_FILE: &str = "enabled";
const FUNDING_FILE: &str = "funding_address";
const PIN_FILE: &str = "funding_pinned";

/// Fee ceiling for one inscription transaction.
pub const MAX_TX_FEE: u64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    pub channel_id: [u8; 32],
    pub sequencer_key: [u8; 32],
}

/// A room's publishing settings on disk.
pub struct PublishDir {
    dir: PathBuf,
}

impl PublishDir {
    pub fn new(room_dir: &Path) -> Self {
        Self {
            dir: room_dir.join("publish"),
        }
    }

    pub fn enabled(&self) -> bool {
        self.dir.join(ENABLED_FILE).exists()
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let path = self.dir.join(ENABLED_FILE);

        if enabled {
            fs::create_dir_all(&self.dir)
                .and_then(|()| fs::write(&path, b"1"))
                .map_err(|error| format!("failed to save the publish setting: {error}"))
        } else {
            // A new start picks the funding address again.
            let _ = fs::remove_file(self.dir.join(FUNDING_FILE));

            match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("failed to save the publish setting: {error}")),
            }
        }
    }

    /// The wallet address publishing was started with, if any.
    pub fn funding_address(&self) -> Option<String> {
        fs::read_to_string(self.dir.join(FUNDING_FILE))
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    }

    /// The funding address the user picked by hand, if any. Unlike the
    /// automatic choice it survives turning publishing off.
    pub fn pinned_funding(&self) -> Option<String> {
        fs::read_to_string(self.dir.join(PIN_FILE))
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    }

    /// Pins `address`, or goes back to the automatic choice when it is `None`.
    pub fn pin_funding(&self, address: Option<&str>) -> Result<(), String> {
        let path = self.dir.join(PIN_FILE);

        match address {
            Some(address) => fs::create_dir_all(&self.dir)
                .and_then(|()| fs::write(&path, address))
                .map_err(|error| format!("failed to save the funding address: {error}")),
            None => match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("failed to save the funding address: {error}")),
            },
        }
    }

    pub fn set_funding_address(&self, address: &str) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .and_then(|()| fs::write(self.dir.join(FUNDING_FILE), address))
            .map_err(|error| format!("failed to save the funding address: {error}"))
    }

    /// The keys if they exist; never creates them.
    pub fn keys_if_present(&self) -> Option<Keys> {
        Some(Keys {
            channel_id: self.read(CHANNEL_FILE).ok()??,
            sequencer_key: self.read(KEY_FILE).ok()??,
        })
    }

    /// The channel and sequencer key, created on first use. The key is a
    /// secret: the file is readable by the owner only.
    pub fn keys(&self) -> Result<Keys, String> {
        let channel = self.read(CHANNEL_FILE)?;
        let key = self.read(KEY_FILE)?;

        match (channel, key) {
            (Some(channel_id), Some(sequencer_key)) => Ok(Keys {
                channel_id,
                sequencer_key,
            }),
            (None, None) => {
                let keys = Keys {
                    channel_id: random_32(),
                    sequencer_key: random_32(),
                };

                self.write(&keys)?;

                Ok(keys)
            }
            _ => Err(format!(
                "{} holds only one of channel_id and sequencer_key; \
                 import both or remove the folder to start over",
                self.dir.display()
            )),
        }
    }

    /// Replaces the room's channel and key with ones the user already has.
    /// Each argument is either 64 hex characters or the path of a file
    /// holding them. Refuses while publishing is on, so a running publisher
    /// never changes identity underneath its own history.
    pub fn import(&self, channel: &str, key: &str) -> Result<Keys, String> {
        if self.enabled() {
            return Err("turn publishing off before importing a channel".to_string());
        }

        let keys = Keys {
            channel_id: hex_or_file(channel)?,
            sequencer_key: hex_or_file(key)?,
        };

        self.write(&keys)?;

        Ok(keys)
    }

    fn read(&self, name: &str) -> Result<Option<[u8; 32]>, String> {
        let path = self.dir.join(name);

        match fs::read_to_string(&path) {
            Ok(text) => parse_hex_32(text.trim())
                .map(Some)
                .map_err(|error| format!("{}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("failed to read {}: {error}", path.display())),
        }
    }

    fn write(&self, keys: &Keys) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .map_err(|error| format!("failed to create {}: {error}", self.dir.display()))?;

        write_private(&self.dir.join(CHANNEL_FILE), &hex::encode(keys.channel_id))?;
        write_private(&self.dir.join(KEY_FILE), &hex::encode(keys.sequencer_key))
    }
}

fn random_32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;

    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;

        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
    }
    .map_err(|error| format!("failed to write {}: {error}", path.display()))?;

    #[cfg(not(unix))]
    let mut file = fs::File::create(path)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;

    file.write_all(text.as_bytes())
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

pub fn parse_hex_32(value: &str) -> Result<[u8; 32], String> {
    let value = value.strip_prefix("0x").unwrap_or(value);

    let bytes = hex::decode(value).map_err(|error| format!("invalid hex: {error}"))?;

    bytes
        .try_into()
        .map_err(|_| "must contain exactly 32 bytes (64 hex characters)".to_string())
}

fn hex_or_file(value: &str) -> Result<[u8; 32], String> {
    let value = value.trim();

    if let Ok(bytes) = parse_hex_32(value) {
        return Ok(bytes);
    }

    let path = Path::new(value);

    if value.is_empty() || !path.is_file() {
        return Err(format!(
            "{value:?} is neither 64 hex characters nor the path of a file"
        ));
    }

    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;

    parse_hex_32(text.trim()).map_err(|error| format!("{}: {error}", path.display()))
}

// ---------------------------------------------------------------------------
// Wallet
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletError {
    /// Basecamp's blockchain module is not running.
    NodeOffline,
    Other(String),
}

/// The Basecamp wallet, as far as publishing needs it.
pub trait Wallet {
    /// Every address the wallet knows (64 hex characters each).
    fn addresses(&self) -> Result<Vec<String>, WalletError>;

    /// Spendable balance of `address`; an unknown address has none.
    fn balance(&self, address: &str) -> Result<u128, WalletError>;

    /// What the wallet literally answered for `address` (reply or error
    /// text). Used only to explain a zero balance.
    fn raw_balance(&self, _address: &str) -> Option<String> {
        None
    }
}

/// The blockchain module inside Basecamp.
pub struct BasecampWallet;

impl BasecampWallet {
    fn client() -> logos_blockchain_client::BlockchainModuleClient {
        logos_blockchain_client::BlockchainModuleClient::bind("blockchain_module")
    }
}

fn wallet_error(error: impl std::fmt::Display) -> WalletError {
    let text = error.to_string();

    if text.contains("not running") {
        WalletError::NodeOffline
    } else {
        WalletError::Other(text)
    }
}

impl Wallet for BasecampWallet {
    fn raw_balance(&self, address: &str) -> Option<String> {
        let text = match Self::client().wallet_get_balance(address) {
            Ok(reply) => format!("reply {}", reply),
            Err(error) => format!("error {}", error),
        };

        Some(text.chars().take(160).collect())
    }

    fn addresses(&self) -> Result<Vec<String>, WalletError> {
        let reply = Self::client()
            .wallet_get_known_addresses()
            .map_err(wallet_error)?;

        let found = all_addresses_in(&reply);

        if found.is_empty() && !reply_is_empty(&reply) {
            // Something came back that is not an address list: say what, so
            // a format difference is visible instead of looking like "no
            // wallet".
            return Err(WalletError::Other(format!(
                "unrecognized wallet reply: {}",
                reply.to_string().chars().take(160).collect::<String>()
            )));
        }

        Ok(found)
    }

    fn balance(&self, address: &str) -> Result<u128, WalletError> {
        match Self::client().wallet_get_balance(address) {
            Ok(reply) => balance_in(&reply),
            Err(error) => {
                let text = error.to_string();

                // A funding address the node has never seen has no balance.
                if text
                    .to_ascii_lowercase()
                    .contains("unknown funding address")
                    || text.to_ascii_lowercase().contains("not found")
                {
                    Ok(0)
                } else {
                    Err(wallet_error(error))
                }
            }
        }
    }
}

/// The module returns a list of hex strings. The reply may also arrive
/// wrapped in an object, as a list of objects, or as JSON text, so collect
/// every 64-hex-character string anywhere in it, in order, without repeats.
fn all_addresses_in(reply: &Value) -> Vec<String> {
    let mut found = Vec::new();

    collect_addresses(reply, 0, &mut found);

    found
}

#[cfg(test)]
fn first_address_in(reply: &Value) -> Option<String> {
    all_addresses_in(reply).into_iter().next()
}

fn collect_addresses(value: &Value, depth: usize, found: &mut Vec<String>) {
    if depth > 4 {
        return;
    }

    match value {
        Value::String(text) => {
            let trimmed = text.trim().trim_start_matches("0x");

            if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                let address = trimmed.to_ascii_lowercase();

                if !found.contains(&address) {
                    found.push(address);
                }

                return;
            }

            // JSON text such as "[\"ab..\"]".
            if let Some(inner) = serde_json::from_str::<Value>(text)
                .ok()
                .filter(|inner| !inner.is_string())
            {
                collect_addresses(&inner, depth + 1, found);
            }
        }
        Value::Array(list) => list
            .iter()
            .for_each(|item| collect_addresses(item, depth + 1, found)),
        Value::Object(map) => map
            .values()
            .for_each(|item| collect_addresses(item, depth + 1, found)),
        _ => {}
    }
}

/// True when a reply holds nothing at all (no addresses), as opposed to
/// something we could not read.
fn reply_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => {
            let text = text.trim();

            text.is_empty()
                || serde_json::from_str::<Value>(text)
                    .ok()
                    .is_some_and(|inner| !inner.is_string() && reply_is_empty(&inner))
        }
        Value::Array(list) => list.is_empty(),
        Value::Object(map) => map.values().all(reply_is_empty),
        _ => false,
    }
}

fn balance_in(reply: &Value) -> Result<u128, WalletError> {
    match reply {
        Value::Object(map) => {
            if map.get("success") == Some(&Value::Bool(false)) {
                let error = map
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown wallet error");

                if error
                    .to_ascii_lowercase()
                    .contains("unknown wallet address")
                {
                    return Ok(0);
                }

                return Err(WalletError::Other(error.to_string()));
            }

            let value = map
                .get("value")
                .or_else(|| map.get("balance"))
                .ok_or_else(|| WalletError::Other(format!("missing balance field: {reply}")))?;

            balance_in(value)
        }

        Value::String(text) => {
            if let Ok(inner) = serde_json::from_str::<Value>(text) {
                if !inner.is_string() {
                    return balance_in(&inner);
                }
            }

            text.trim()
                .parse::<u128>()
                .map_err(|e| WalletError::Other(format!("invalid balance {text:?}: {e}")))
        }

        Value::Number(number) => number
            .to_string()
            .parse::<u128>()
            .map_err(|e| WalletError::Other(e.to_string())),

        _ => Err(WalletError::Other(format!(
            "unexpected balance reply: {reply}"
        ))),
    }
}

/// The address that pays publishing fees, and its balance.
pub struct Funding {
    pub address: Result<Option<String>, WalletError>,
    pub balance: Option<Result<u128, WalletError>>,
    /// What the wallet said about each address it was asked about:
    /// (address, balance or error text). Shown in the status for diagnosis.
    pub checked: Vec<(String, String)>,
}

fn describe_for(wallet: &dyn Wallet, address: &str, result: &Result<u128, WalletError>) -> String {
    match result {
        Ok(0) => match wallet.raw_balance(address) {
            Some(raw) => format!("0 (wallet said: {raw})"),
            None => "0".to_string(),
        },
        other => describe_balance(other),
    }
}

fn describe_balance(result: &Result<u128, WalletError>) -> String {
    match result {
        Ok(funds) => funds.to_string(),
        Err(WalletError::NodeOffline) => "error: node offline".to_string(),
        Err(WalletError::Other(text)) => format!("error: {text}"),
    }
}

/// Picks the funding address. The wallet holds several keys (mining rewards,
/// staking, ...), and only some of them hold funds, so the one already in use
/// (`saved`) stays; otherwise the one with the most funds wins, the first
/// listed on a tie.
pub fn choose_funding(wallet: &dyn Wallet, saved: Option<&str>) -> Funding {
    let list = match wallet.addresses() {
        Ok(list) => list,
        Err(error) => {
            return Funding {
                address: Err(error),
                balance: None,
                checked: Vec::new(),
            }
        }
    };

    if list.is_empty() {
        return Funding {
            address: Ok(None),
            balance: None,
            checked: Vec::new(),
        };
    }

    if let Some(saved) = saved.filter(|saved| list.iter().any(|address| address == saved)) {
        let balance = wallet.balance(saved);

        return Funding {
            address: Ok(Some(saved.to_string())),
            checked: vec![(saved.to_string(), describe_for(wallet, saved, &balance))],
            balance: Some(balance),
        };
    }

    let mut best: Option<(String, u128)> = None;
    let mut first_error = None;
    let mut checked = Vec::new();

    for address in &list {
        let balance = wallet.balance(address);

        checked.push((address.clone(), describe_for(wallet, address, &balance)));

        match balance {
            Ok(funds) if best.as_ref().is_none_or(|(_, most)| funds > *most) => {
                best = Some((address.clone(), funds));
            }
            Ok(_) => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }

    match (best, first_error) {
        (Some((address, funds)), _) => Funding {
            address: Ok(Some(address)),
            balance: Some(Ok(funds)),
            checked,
        },
        (None, Some(error)) => Funding {
            address: Ok(Some(list[0].clone())),
            balance: Some(Err(error)),
            checked,
        },
        (None, None) => Funding {
            address: Ok(Some(list[0].clone())),
            balance: Some(Ok(0)),
            checked,
        },
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Everything the status depends on.
pub struct Inputs<'a> {
    pub enabled: bool,
    /// False for a guest in a shared room: only the host publishes.
    pub is_host: bool,
    /// The publisher is running now.
    pub running: bool,
    pub wallet_address: Result<Option<String>, WalletError>,
    pub balance: Option<Result<u128, WalletError>>,
    /// Per-address balances the wallet reported, for the status.
    pub checked: &'a [(String, String)],
    /// The funding address was picked by hand.
    pub pinned: bool,
    pub head: Option<u64>,
    pub progress: Option<&'a PublicationState>,
}

/// The five things the button area can say.
pub fn state_name(inputs: &Inputs) -> &'static str {
    if !inputs.is_host {
        return "guest";
    }

    if !inputs.enabled {
        return "off";
    }

    if inputs.progress.is_some_and(|state| state.failed.is_some()) {
        return "error";
    }

    match &inputs.wallet_address {
        Err(_) | Ok(None) => return "needs_node",
        Ok(Some(_)) => {}
    }

    // A balance that cannot be read (the node may still be catching up) is
    // no proof of an empty wallet: once the publisher runs, trust it.
    let unreadable = inputs.running && matches!(inputs.balance, Some(Err(_)));

    if !unreadable && !matches!(inputs.balance, Some(Ok(balance)) if balance > 0) {
        return "needs_funding";
    }

    let Some(progress) = inputs.progress else {
        return "syncing";
    };

    let caught_up = inputs.head.is_none_or(|head| progress.next_height > head);

    let settling = progress.entries.values().any(|entry| {
        matches!(
            entry.status,
            PublicationStatus::Submitted | PublicationStatus::Mempool | PublicationStatus::Orphaned
        )
    });

    if inputs.running && caught_up && !settling {
        "published"
    } else {
        "syncing"
    }
}

pub fn status_json(inputs: &Inputs, keys: Option<&Keys>) -> Value {
    let state = state_name(inputs);

    let (on_chain, finalized) = inputs.progress.map_or((0, 0), |progress| {
        let count = |wanted: PublicationStatus| {
            progress
                .entries
                .values()
                .filter(|entry| entry.status == wanted)
                .count()
        };

        (
            count(PublicationStatus::OnChain),
            count(PublicationStatus::Finalized),
        )
    });

    let message = match state {
        "guest" => "Only the host of a shared room publishes it.".to_string(),
        "off" => "Publishing is off. This chain is a local sandbox.".to_string(),
        "needs_node" => match &inputs.wallet_address {
            Err(WalletError::NodeOffline) => {
                "Start the Logos blockchain node in Basecamp to publish.".to_string()
            }
            Err(WalletError::Other(error)) => format!("Wallet unavailable: {error}"),
            _ => "The Basecamp wallet has no address yet; create one first.".to_string(),
        },
        "needs_funding" => match &inputs.balance {
            Some(Err(WalletError::NodeOffline)) => {
                "Start the Logos blockchain node in Basecamp to read the wallet balance.".to_string()
            }
            Some(Err(WalletError::Other(error))) => format!(
                "Could not read the wallet balance yet: {error}. The node may still be catching up; pick the address that holds your funds below."
            ),
            _ => "Add funds to the wallet address to pay publishing fees.".to_string(),
        },
        "error" => inputs
            .progress
            .and_then(|progress| progress.failed.clone())
            .unwrap_or_default(),
        "syncing" => "Publishing blocks to Logos…".to_string(),
        _ => "Published to Logos.".to_string(),
    };

    json!({
        "state": state,
        "message": message,
        "enabled": inputs.enabled,
        "running": inputs.running,
        "channelId": keys.map(|keys| hex::encode(keys.channel_id)),
        "walletAddress": inputs.wallet_address.as_ref().ok().cloned().flatten(),
        "balance": inputs
            .balance
            .as_ref()
            .and_then(|balance| balance.as_ref().ok())
            .map(|balance| balance.to_string()),
        "balanceError": inputs
            .balance
            .as_ref()
            .and_then(|balance| balance.as_ref().err())
            .map(|error| match error {
                WalletError::NodeOffline => "node offline".to_string(),
                WalletError::Other(text) => text.clone(),
            }),
        "pinned": inputs.pinned,
        "wallet": inputs
            .checked
            .iter()
            .map(|(address, balance)| json!({ "address": address, "balance": balance }))
            .collect::<Vec<_>>(),
        "head": inputs.head,
        "nextHeight": inputs.progress.map(|progress| progress.next_height),
        "onChain": on_chain,
        "finalized": finalized,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use solizone_evm::publication::PublicationEntry;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("solicamp-publish-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn address() -> String {
        "ab".repeat(32)
    }

    fn base<'a>() -> Inputs<'a> {
        Inputs {
            enabled: true,
            is_host: true,
            running: true,
            wallet_address: Ok(Some(address())),
            balance: Some(Ok(100)),
            checked: &[],
            pinned: false,
            head: Some(10),
            progress: None,
        }
    }

    fn progress(next_height: u64, status: PublicationStatus) -> PublicationState {
        let mut entries = BTreeMap::new();

        entries.insert(
            next_height.saturating_sub(1),
            PublicationEntry {
                block_hash: "h".into(),
                tx_hash: "t".into(),
                status,
            },
        );

        PublicationState {
            next_height,
            entries,
            failed: None,
            sequencer_checkpoint: None,
        }
    }

    #[test]
    fn keys_are_created_once_and_kept() {
        let room = temp_dir("keys");
        let dir = PublishDir::new(&room);

        let first = dir.keys().unwrap();
        let again = dir.keys().unwrap();

        assert_eq!(first, again);
        assert_ne!(first.channel_id, first.sequencer_key);

        // A different room gets a different channel.
        let other = PublishDir::new(&temp_dir("keys-other")).keys().unwrap();
        assert_ne!(first.channel_id, other.channel_id);
    }

    #[cfg(unix)]
    #[test]
    fn the_sequencer_key_is_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;

        let room = temp_dir("mode");
        PublishDir::new(&room).keys().unwrap();

        let mode = fs::metadata(room.join("publish").join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();

        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn imports_hex_or_files_and_refuses_while_publishing() {
        let room = temp_dir("import");
        let dir = PublishDir::new(&room);

        let key_file = room.join("key.hex");
        fs::write(&key_file, format!("0x{}\n", "22".repeat(32))).unwrap();

        let keys = dir
            .import(&"11".repeat(32), key_file.to_str().unwrap())
            .unwrap();

        assert_eq!(keys.channel_id, [0x11; 32]);
        assert_eq!(keys.sequencer_key, [0x22; 32]);
        assert_eq!(dir.keys().unwrap(), keys);

        dir.set_enabled(true).unwrap();
        assert!(dir.import(&"33".repeat(32), &"44".repeat(32)).is_err());

        dir.set_enabled(false).unwrap();
        assert!(dir.import("not hex", &"44".repeat(32)).is_err());
        assert!(dir.import(&"33".repeat(32), "/no/such/file").is_err());
        assert_eq!(dir.keys().unwrap(), keys);
    }

    #[test]
    fn the_enabled_flag_persists() {
        let dir = PublishDir::new(&temp_dir("flag"));

        assert!(!dir.enabled());
        dir.set_enabled(true).unwrap();
        assert!(dir.enabled());
        dir.set_enabled(false).unwrap();
        dir.set_enabled(false).unwrap();
        assert!(!dir.enabled());
    }

    #[test]
    fn half_a_key_set_is_reported_not_overwritten() {
        let room = temp_dir("half");
        let dir = PublishDir::new(&room);

        fs::create_dir_all(room.join("publish")).unwrap();
        fs::write(room.join("publish").join(CHANNEL_FILE), "11".repeat(32)).unwrap();

        assert!(dir.keys().unwrap_err().contains("only one"));
    }

    #[test]
    fn walks_through_the_states() {
        let mut inputs = base();
        inputs.enabled = false;
        assert_eq!(state_name(&inputs), "off");

        inputs.enabled = true;
        inputs.is_host = false;
        assert_eq!(state_name(&inputs), "guest");

        inputs.is_host = true;
        inputs.wallet_address = Err(WalletError::NodeOffline);
        assert_eq!(state_name(&inputs), "needs_node");

        inputs.wallet_address = Ok(None);
        assert_eq!(state_name(&inputs), "needs_node");

        inputs.wallet_address = Ok(Some(address()));
        inputs.balance = Some(Ok(0));
        assert_eq!(state_name(&inputs), "needs_funding");

        inputs.balance = Some(Ok(5));
        assert_eq!(state_name(&inputs), "syncing");

        let behind = progress(4, PublicationStatus::OnChain);
        inputs.progress = Some(&behind);
        assert_eq!(state_name(&inputs), "syncing");

        let pending = progress(11, PublicationStatus::Mempool);
        inputs.progress = Some(&pending);
        assert_eq!(state_name(&inputs), "syncing");

        let done = progress(11, PublicationStatus::OnChain);
        inputs.progress = Some(&done);
        assert_eq!(state_name(&inputs), "published");

        inputs.running = false;
        assert_eq!(state_name(&inputs), "syncing");

        inputs.running = true;
        let mut failed = progress(11, PublicationStatus::OnChain);
        failed.failed = Some("block too large".into());
        inputs.progress = Some(&failed);
        assert_eq!(state_name(&inputs), "error");
    }

    #[test]
    fn status_json_carries_what_the_ui_shows() {
        let keys = Keys {
            channel_id: [1; 32],
            sequencer_key: [2; 32],
        };
        let done = progress(11, PublicationStatus::OnChain);
        let mut inputs = base();
        inputs.progress = Some(&done);

        let status = status_json(&inputs, Some(&keys));

        assert_eq!(status["state"], "published");
        assert_eq!(status["channelId"], "01".repeat(32));
        assert_eq!(status["walletAddress"], address());
        assert_eq!(status["balance"], "100");
        assert_eq!(status["onChain"], 1);
        // The sequencer key never leaves the module.
        assert!(!status.to_string().contains(&"02".repeat(32)));
    }

    struct FakeWallet {
        list: Result<Vec<String>, WalletError>,
        funds: Vec<(String, u128)>,
    }

    impl Wallet for FakeWallet {
        fn addresses(&self) -> Result<Vec<String>, WalletError> {
            self.list.clone()
        }

        fn balance(&self, address: &str) -> Result<u128, WalletError> {
            Ok(self
                .funds
                .iter()
                .find(|(known, _)| known == address)
                .map_or(0, |(_, funds)| *funds))
        }
    }

    fn wallet(funds: &[(&str, u128)]) -> FakeWallet {
        FakeWallet {
            list: Ok(funds
                .iter()
                .map(|(address, _)| address.to_string())
                .collect()),
            funds: funds.iter().map(|(a, f)| (a.to_string(), *f)).collect(),
        }
    }

    #[test]
    fn the_funded_address_is_chosen_not_just_the_first() {
        let (blend, pow, stake) = ("aa".repeat(32), "bb".repeat(32), "cc".repeat(32));
        let wallet = wallet(&[(&blend, 0), (&pow, 49), (&stake, 7)]);

        let funding = choose_funding(&wallet, None);
        assert_eq!(funding.address, Ok(Some(pow.clone())));
        assert_eq!(funding.balance, Some(Ok(49)));
        assert_eq!(funding.checked.len(), 3);
        assert_eq!(funding.checked[1], (pow.clone(), "49".to_string()));

        // The address already in use stays, even if another now holds more.
        let funding = choose_funding(&wallet, Some(&stake));
        assert_eq!(funding.address, Ok(Some(stake)));
        assert_eq!(funding.balance, Some(Ok(7)));

        // An unknown saved address is ignored.
        let funding = choose_funding(&wallet, Some("zz"));
        assert_eq!(funding.address, Ok(Some(pow)));

        // Nothing funded: the first address, with a zero balance.
        let empty = self::wallet(&[(&blend, 0)]);
        let funding = choose_funding(&empty, None);
        assert_eq!(funding.address, Ok(Some(blend)));
        assert_eq!(funding.balance, Some(Ok(0)));

        // No addresses, and an offline node.
        assert_eq!(choose_funding(&self::wallet(&[]), None).address, Ok(None));

        let offline = FakeWallet {
            list: Err(WalletError::NodeOffline),
            funds: vec![],
        };
        assert_eq!(
            choose_funding(&offline, None).address,
            Err(WalletError::NodeOffline)
        );
    }

    #[test]
    fn the_funding_choice_is_remembered_until_publishing_stops() {
        let dir = PublishDir::new(&temp_dir("funding"));

        assert_eq!(dir.funding_address(), None);
        dir.set_funding_address("ab").unwrap();
        assert_eq!(dir.funding_address(), Some("ab".to_string()));
        dir.set_enabled(false).unwrap();
        assert_eq!(dir.funding_address(), None);
    }

    #[test]
    fn collects_every_address_once() {
        let (a, b) = ("ab".repeat(32), "cd".repeat(32));

        assert_eq!(
            all_addresses_in(&json!([a, format!("0x{b}"), a, "short"])),
            vec![a, b]
        );
    }

    #[test]
    fn reads_wallet_replies_leniently() {
        let hex_address = "AB".repeat(32);

        assert_eq!(
            first_address_in(&json!([format!("0x{hex_address}")])),
            Some(address())
        );
        assert_eq!(
            first_address_in(&json!({ "value": [hex_address] })),
            Some(address())
        );
        assert_eq!(
            first_address_in(&json!(format!("[\"{hex_address}\"]"))),
            Some(address())
        );
        assert_eq!(
            first_address_in(&json!([{ "address": hex_address }])),
            Some(address())
        );
        assert!(reply_is_empty(&json!([])));
        assert!(reply_is_empty(&json!("[]")));
        assert!(reply_is_empty(&json!({ "value": [] })));
        assert!(!reply_is_empty(&json!(["short"])));
        assert_eq!(first_address_in(&json!([])), None);
        assert_eq!(first_address_in(&json!(["short"])), None);

        assert_eq!(balance_in(&json!("1500")), Ok(1500));
        assert_eq!(balance_in(&json!(7)), Ok(7));
        assert_eq!(balance_in(&json!({ "value": "9" })), Ok(9));
        assert!(balance_in(&json!("oops")).is_err());
        assert_eq!(
            balance_in(&json!({
                "error": null,
                "success": true,
                "value": "4916828751"
            })),
            Ok(4916828751)
        );
    }

    #[test]
    fn a_picked_funding_address_is_kept_until_cleared() {
        let dir = PublishDir::new(&temp_dir("pin"));

        assert_eq!(dir.pinned_funding(), None);

        dir.pin_funding(Some(&address())).unwrap();
        assert_eq!(dir.pinned_funding(), Some(address()));

        // Turning publishing off forgets the automatic choice, not the pick.
        dir.set_funding_address("cd").unwrap();
        dir.set_enabled(true).unwrap();
        dir.set_enabled(false).unwrap();
        assert_eq!(dir.funding_address(), None);
        assert_eq!(dir.pinned_funding(), Some(address()));

        dir.pin_funding(None).unwrap();
        assert_eq!(dir.pinned_funding(), None);
        dir.pin_funding(None).unwrap();
    }

    #[test]
    fn an_unreadable_balance_is_reported_not_shown_as_zero() {
        let mut inputs = base();
        inputs.running = false;
        inputs.balance = Some(Err(WalletError::Other("Ledger state not found".into())));

        assert_eq!(state_name(&inputs), "needs_funding");

        let status = status_json(&inputs, None);

        assert!(status["message"]
            .as_str()
            .unwrap()
            .contains("Ledger state not found"));
        assert_eq!(status["balanceError"], "Ledger state not found");
        assert!(status["balance"].is_null());

        // A running publisher is not stopped by a failed read.
        inputs.running = true;
        assert_eq!(state_name(&inputs), "syncing");
    }
}
