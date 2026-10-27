//! Working together in a room.
//!
//! One person **hosts** a room: their node orders transactions and produces
//! blocks. Everyone else is a **guest**: their node is a follower that
//! replays and verifies every block the host publishes. Guests send their
//! signed transactions to the host; nobody trusts anybody:
//!
//! * a guest executes every block itself, so a host cannot fake state;
//! * blocks are signed by the host key named in the invite, so another room
//!   member cannot substitute a different chain;
//! * all traffic is sealed with a key that only travels inside the invite,
//!   because the messaging network itself is public.
//!
//! This module has no networking. [`Session`] turns incoming payloads into
//! outgoing payloads; whatever carries them (Logos Messaging, a test bus)
//! is the caller's business. Messages may be lost, duplicated or reordered:
//! a guest simply asks again (`hello`) from the height it is missing.

use alloy_primitives::{keccak256, Address, Signature, B256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Value};
use solizone_evm::embedded::EmbeddedNode;

/// Largest sealed payload accepted or produced.
const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
/// Blocks sent in answer to one `hello`.
pub const DEFAULT_BLOCKS_PER_REPLY: u64 = 32;
const INVITE_PREFIX: &str = "solicamp1:";
const SIGNATURE_DOMAIN: &[u8] = b"solicamp/block/v1";

// ---------------------------------------------------------------------------
// Invite
// ---------------------------------------------------------------------------

/// Everything a guest needs to join: which chain, which key, whose blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub room_id: String,
    pub name: String,
    pub chain_id: u64,
    /// Secret that seals all room traffic. Share the invite only with people
    /// you want in the room.
    pub key: [u8; 32],
    /// The only address whose block signatures are accepted.
    pub host: Address,
}

impl Invite {
    pub fn encode(&self) -> String {
        let body = json!({
            "v": 1,
            "room": self.room_id,
            "name": self.name,
            "chain": self.chain_id,
            "key": hex::encode(self.key),
            "host": self.host.to_string(),
        });

        format!("{INVITE_PREFIX}{}", hex::encode(body.to_string()))
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let body = text
            .trim()
            .strip_prefix(INVITE_PREFIX)
            .ok_or("this is not a SoliCamp invite")?;

        let bytes = hex::decode(body).map_err(|_| "the invite is damaged".to_string())?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "the invite is damaged".to_string())?;

        if value["v"] != 1 {
            return Err("this invite is from a newer SoliCamp".to_string());
        }

        let key: [u8; 32] = hex::decode(value["key"].as_str().unwrap_or(""))
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or("the invite's key is damaged")?;

        let room_id = value["room"].as_str().ok_or("the invite has no room")?;

        // The room id becomes a folder name and part of a topic.
        if room_id.is_empty()
            || room_id.len() > 32
            || !room_id.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return Err("the invite has an invalid room id".to_string());
        }

        Ok(Self {
            room_id: room_id.to_string(),
            name: value["name"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(60)
                .collect(),
            chain_id: value["chain"]
                .as_u64()
                .ok_or("the invite has no chain id")?,
            key,
            host: value["host"]
                .as_str()
                .and_then(|text| text.parse().ok())
                .ok_or("the invite has no valid host address")?,
        })
    }

    /// The messaging content topic of this room (LIP-23 shape).
    pub fn topic(&self) -> String {
        format!("/solicamp/1/room-{}/proto", self.room_id)
    }
}

/// A new random 256-bit room key.
pub fn new_room_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

// ---------------------------------------------------------------------------
// Sealing
// ---------------------------------------------------------------------------

fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));

    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);

    let mut out = nonce.to_vec();
    out.extend(
        cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| "encryption failed".to_string())?,
    );

    if out.len() > MAX_PAYLOAD {
        return Err("message too large".to_string());
    }

    Ok(out)
}

fn open(key: &[u8; 32], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() <= 12 || sealed.len() > MAX_PAYLOAD {
        return None;
    }

    let (nonce, body) = sealed.split_at(12);

    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(Nonce::from_slice(nonce), body)
        .ok()
}

// ---------------------------------------------------------------------------
// Host signature
// ---------------------------------------------------------------------------

fn block_digest(chain_id: u64, height: u64, block: &[u8]) -> B256 {
    let mut data = SIGNATURE_DOMAIN.to_vec();
    data.extend(chain_id.to_be_bytes());
    data.extend(height.to_be_bytes());
    data.extend(block);

    keccak256(data)
}

/// A fresh host key; persist it with [`signer_to_hex`].
pub fn new_host_key() -> PrivateKeySigner {
    PrivateKeySigner::random()
}

pub fn signer_to_hex(signer: &PrivateKeySigner) -> String {
    hex::encode(signer.to_bytes())
}

pub fn signer_from_hex(text: &str) -> Result<PrivateKeySigner, String> {
    let bytes: [u8; 32] = hex::decode(text.trim())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("the stored host key is damaged")?;

    PrivateKeySigner::from_bytes(&B256::from(bytes))
        .map_err(|error| format!("the stored host key is invalid: {error}"))
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

enum Role {
    Host {
        signer: PrivateKeySigner,
        /// First height not yet broadcast.
        broadcast_next: u64,
    },
    Guest,
}

/// What handling one incoming message produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Payloads to publish on the room topic.
    pub send: Vec<Vec<u8>>,
    /// Height of the block a guest just applied.
    pub applied: Option<u64>,
    /// Something worth showing to the user (a refused block, say).
    pub note: Option<String>,
}

pub struct Session {
    invite: Invite,
    role: Role,
    blocks_per_reply: u64,
}

impl Session {
    /// The host side. `node` must be the room's producing node.
    pub fn host(
        invite: Invite,
        signer: PrivateKeySigner,
        node: &EmbeddedNode,
    ) -> Result<Self, String> {
        if signer.address() != invite.host {
            return Err("the host key does not match the invite".to_string());
        }

        Ok(Self {
            invite,
            role: Role::Host {
                signer,
                broadcast_next: node.next_height(),
            },
            blocks_per_reply: DEFAULT_BLOCKS_PER_REPLY,
        })
    }

    pub fn guest(invite: Invite) -> Self {
        Self {
            invite,
            role: Role::Guest,
            blocks_per_reply: DEFAULT_BLOCKS_PER_REPLY,
        }
    }

    pub fn invite(&self) -> &Invite {
        &self.invite
    }

    pub fn is_host(&self) -> bool {
        matches!(self.role, Role::Host { .. })
    }

    fn pack(&self, message: Value) -> Result<Vec<u8>, String> {
        seal(&self.invite.key, message.to_string().as_bytes())
    }

    fn unpack(&self, payload: &[u8]) -> Option<Value> {
        serde_json::from_slice(&open(&self.invite.key, payload)?).ok()
    }

    /// A guest asking for the blocks it is missing. Send it when joining, and
    /// again every few seconds while behind.
    pub fn hello(&self, node: &EmbeddedNode) -> Result<Vec<u8>, String> {
        self.pack(json!({ "t": "hello", "next": node.next_height() }))
    }

    /// A guest handing a signed transaction (raw EIP-2718 bytes) to the host.
    pub fn submit(&self, raw_transaction: &[u8]) -> Result<Vec<u8>, String> {
        self.pack(json!({ "t": "tx", "raw": hex::encode(raw_transaction) }))
    }

    fn block_message(&self, node: &EmbeddedNode, height: u64) -> Result<Option<Vec<u8>>, String> {
        let Role::Host { signer, .. } = &self.role else {
            return Ok(None);
        };

        let Some(block) = node.block_bytes(height)? else {
            return Ok(None);
        };

        let signature = signer
            .sign_hash_sync(&block_digest(self.invite.chain_id, height, &block))
            .map_err(|error| format!("signing failed: {error}"))?;

        self.pack(json!({
            "t": "block",
            "h": height,
            "b": hex::encode(&block),
            "s": hex::encode(signature.as_bytes()),
        }))
        .map(Some)
    }

    /// Host: blocks produced since the last call, ready to publish.
    pub fn poll(&mut self, node: &EmbeddedNode) -> Result<Vec<Vec<u8>>, String> {
        let Role::Host { broadcast_next, .. } = &self.role else {
            return Ok(Vec::new());
        };

        let (from, to) = (*broadcast_next, node.next_height());
        let mut out = Vec::new();

        for height in from..to {
            if let Some(message) = self.block_message(node, height)? {
                out.push(message);
            }
        }

        if let Role::Host { broadcast_next, .. } = &mut self.role {
            *broadcast_next = to.max(from);
        }

        Ok(out)
    }

    /// Handles one payload from the room topic. Anything that is not valid
    /// for this room (wrong key, damaged, wrong sender) is ignored.
    pub fn handle(&mut self, node: &EmbeddedNode, payload: &[u8]) -> Outcome {
        let Some(message) = self.unpack(payload) else {
            return Outcome::default();
        };

        match (&self.role, message["t"].as_str()) {
            (Role::Host { .. }, Some("hello")) => self.answer_hello(node, &message),
            (Role::Host { .. }, Some("tx")) => self.accept_transaction(node, &message),
            (Role::Guest, Some("block")) => self.apply_block(node, &message),
            _ => Outcome::default(),
        }
    }

    fn answer_hello(&self, node: &EmbeddedNode, message: &Value) -> Outcome {
        let Some(next) = message["next"].as_u64() else {
            return Outcome::default();
        };

        let to = node
            .next_height()
            .min(next.saturating_add(self.blocks_per_reply));
        let mut send = Vec::new();

        for height in next..to {
            match self.block_message(node, height) {
                Ok(Some(block)) => send.push(block),
                Ok(None) => break,
                Err(error) => {
                    return Outcome {
                        note: Some(error),
                        ..Outcome::default()
                    }
                }
            }
        }

        Outcome {
            send,
            ..Outcome::default()
        }
    }

    fn accept_transaction(&self, node: &EmbeddedNode, message: &Value) -> Outcome {
        let Some(raw) = message["raw"].as_str() else {
            return Outcome::default();
        };

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendRawTransaction",
            "params": [format!("0x{}", raw.trim_start_matches("0x"))],
        });

        let response: Value =
            serde_json::from_str(&node.request(&request.to_string())).unwrap_or(Value::Null);

        let note = response["error"]["message"]
            .as_str()
            .map(|reason| format!("refused a guest's transaction: {reason}"));

        Outcome {
            note,
            ..Outcome::default()
        }
    }

    fn apply_block(&self, node: &EmbeddedNode, message: &Value) -> Outcome {
        let (Some(height), Some(block), Some(signature)) = (
            message["h"].as_u64(),
            message["b"]
                .as_str()
                .and_then(|text| hex::decode(text).ok()),
            message["s"]
                .as_str()
                .and_then(|text| hex::decode(text).ok()),
        ) else {
            return Outcome::default();
        };

        let signer = Signature::from_raw(&signature).ok().and_then(|signature| {
            signature
                .recover_address_from_prehash(&block_digest(self.invite.chain_id, height, &block))
                .ok()
        });

        if signer != Some(self.invite.host) {
            return Outcome {
                note: Some(format!("ignored block #{height}: not signed by the host")),
                ..Outcome::default()
            };
        }

        let next = node.next_height();

        if height < next {
            return Outcome::default(); // already have it
        }

        if height > next {
            // A gap: ask again from where we are.
            return Outcome {
                send: self.hello(node).into_iter().collect(),
                ..Outcome::default()
            };
        }

        match node.apply_block(&block) {
            Ok(applied) => Outcome {
                applied: Some(applied),
                ..Outcome::default()
            },
            Err(error) => Outcome {
                note: Some(format!("refused block #{height}: {error}")),
                ..Outcome::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Bytes, TxKind, U256};
    use solizone_evm::{embedded::EmbeddedConfig, node::dev_accounts::DEV_ACCOUNTS};

    use super::*;

    const CHAIN: u64 = 0x5A00_1234;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "collab-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Peer {
        node: EmbeddedNode,
        session: Session,
        dir: PathBuf,
    }

    impl Drop for Peer {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn invite(host: Address) -> Invite {
        Invite {
            room_id: "abc123".into(),
            name: "Test".into(),
            chain_id: CHAIN,
            key: new_room_key(),
            host,
        }
    }

    fn host_peer(signer: &PrivateKeySigner, invite: &Invite) -> Peer {
        let dir = temp_dir("host");
        let node = EmbeddedNode::start(EmbeddedConfig {
            chain_id: CHAIN,
            block_time: Duration::from_millis(40),
            ..EmbeddedConfig::local(&dir)
        })
        .unwrap();
        let session = Session::host(invite.clone(), signer.clone(), &node).unwrap();

        Peer { node, session, dir }
    }

    fn guest_peer(invite: &Invite) -> Peer {
        let dir = temp_dir("guest");
        let node = EmbeddedNode::start(EmbeddedConfig {
            chain_id: CHAIN,
            follower: true,
            ..EmbeddedConfig::local(&dir)
        })
        .unwrap();

        Peer {
            node,
            session: Session::guest(invite.clone()),
            dir,
        }
    }

    fn signed_deploy(nonce: u64) -> Vec<u8> {
        let signer: PrivateKeySigner = DEV_ACCOUNTS[0].private_key.parse().unwrap();
        let hex = include_str!("../vendor/solizone-evm/contracts/Counter.creation.hex").trim();

        let tx = TxEip1559 {
            chain_id: CHAIN,
            nonce,
            gas_limit: 1_000_000,
            max_fee_per_gas: 0,
            max_priority_fee_per_gas: 0,
            to: TxKind::Create,
            value: U256::ZERO,
            access_list: Default::default(),
            input: Bytes::from(hex::decode(hex.trim_start_matches("0x")).unwrap()),
        };

        let signature = signer.sign_hash_sync(&tx.signature_hash()).unwrap();
        TxEnvelope::from(tx.into_signed(signature)).encoded_2718()
    }

    /// Delivers `payloads` (sent by `from`) to every peer but the sender and
    /// keeps going until nobody has anything more to say.
    fn pump(peers: &mut [&mut Peer], from: usize, payloads: Vec<Vec<u8>>) -> Vec<String> {
        let mut notes = Vec::new();
        let mut queue: std::collections::VecDeque<(usize, Vec<u8>)> =
            payloads.into_iter().map(|p| (from, p)).collect();

        while let Some((sender, payload)) = queue.pop_front() {
            for index in 0..peers.len() {
                if index == sender {
                    continue;
                }

                let peer = &mut *peers[index];
                let outcome = peer.session.handle(&peer.node, &payload);

                notes.extend(outcome.note);
                queue.extend(outcome.send.into_iter().map(|p| (index, p)));
            }
        }

        notes
    }

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);

        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn invites_round_trip_and_reject_damage() {
        let host = new_host_key();
        let original = invite(host.address());

        assert_eq!(Invite::decode(&original.encode()).unwrap(), original);
        assert_eq!(original.topic(), "/solicamp/1/room-abc123/proto");

        assert!(Invite::decode("hello").is_err());
        assert!(Invite::decode("solicamp1:zz").is_err());
        assert!(Invite::decode(&format!("solicamp1:{}", hex::encode("{}"))).is_err());

        let mut bad = original.encode();
        bad.truncate(bad.len() - 6);
        assert!(Invite::decode(&bad).is_err());

        let traversal = Invite {
            room_id: "../../etc".into(),
            ..original
        };
        assert!(Invite::decode(&traversal.encode()).is_err());
    }

    #[test]
    fn host_keys_round_trip_through_hex() {
        let key = new_host_key();
        assert_eq!(
            signer_from_hex(&signer_to_hex(&key)).unwrap().address(),
            key.address()
        );
        assert!(signer_from_hex("nope").is_err());
    }

    #[test]
    fn sealed_messages_need_the_key_and_resist_tampering() {
        let key = new_room_key();
        let sealed = seal(&key, b"hello").unwrap();

        assert_eq!(open(&key, &sealed).unwrap(), b"hello");
        assert!(open(&new_room_key(), &sealed).is_none());

        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&key, &tampered).is_none());

        assert!(open(&key, &[]).is_none());
        assert!(open(&key, &[0; 12]).is_none());

        // Two seals of the same text differ (fresh nonce).
        assert_ne!(seal(&key, b"hello").unwrap(), sealed);
    }

    #[test]
    fn a_room_of_three_works_end_to_end() {
        let signer = new_host_key();
        let invite = invite(signer.address());

        let mut host = host_peer(&signer, &invite);
        let mut alice = guest_peer(&invite);
        let mut bob = guest_peer(&invite);

        // Guests join: hello → the host answers with block #0.
        let hello_a = alice.session.hello(&alice.node).unwrap();
        let hello_b = bob.session.hello(&bob.node).unwrap();

        pump(&mut [&mut host, &mut alice, &mut bob], 1, vec![hello_a]);
        pump(&mut [&mut host, &mut alice, &mut bob], 2, vec![hello_b]);

        assert_eq!(alice.node.head_height(), Some(0));
        assert_eq!(bob.node.head_height(), Some(0));

        // Alice deploys a contract by sending the signed transaction to the host.
        let tx = alice.session.submit(&signed_deploy(0)).unwrap();
        let notes = pump(&mut [&mut host, &mut alice, &mut bob], 1, vec![tx]);
        assert!(notes.is_empty(), "{notes:?}");

        wait_until("the host's block", || host.node.head_height() == Some(1));

        // The host publishes it; everybody converges on the same chain.
        let blocks = host.session.poll(&host.node).unwrap();
        assert_eq!(blocks.len(), 1);
        pump(&mut [&mut host, &mut alice, &mut bob], 0, blocks);

        for guest in [&alice, &bob] {
            assert_eq!(guest.node.head_height(), Some(1));

            for height in 0..=1 {
                assert_eq!(
                    guest.node.block_bytes(height).unwrap(),
                    host.node.block_bytes(height).unwrap()
                );
            }
        }

        // Nothing new: poll is quiet.
        assert!(host.session.poll(&host.node).unwrap().is_empty());
    }

    #[test]
    fn a_late_joiner_catches_up_in_batches() {
        let signer = new_host_key();
        let invite = invite(signer.address());

        let mut host = host_peer(&signer, &invite);
        host.session.blocks_per_reply = 2;

        for nonce in 0..4 {
            let outcome = {
                let tx = Session::guest(invite.clone())
                    .submit(&signed_deploy(nonce))
                    .unwrap();
                host.session.handle(&host.node, &tx)
            };
            assert!(outcome.note.is_none(), "{:?}", outcome.note);
            wait_until("a block", || host.node.head_height() == Some(nonce + 1));
        }

        // Joins after five blocks exist (#0 to #4); replies of two blocks each still get there.
        let mut late = guest_peer(&invite);
        let hello = late.session.hello(&late.node).unwrap();

        pump(&mut [&mut host, &mut late], 1, vec![hello]);
        assert_eq!(
            late.node.head_height(),
            Some(1),
            "first batch: blocks 0 and 1"
        );

        let hello = late.session.hello(&late.node).unwrap();
        pump(&mut [&mut host, &mut late], 1, vec![hello]);
        assert_eq!(
            late.node.head_height(),
            Some(3),
            "second batch: blocks 2 and 3"
        );

        let hello = late.session.hello(&late.node).unwrap();
        pump(&mut [&mut host, &mut late], 1, vec![hello]);

        assert_eq!(late.node.head_height(), Some(4));
        assert_eq!(
            late.node.block_bytes(4).unwrap(),
            host.node.block_bytes(4).unwrap()
        );
    }

    #[test]
    fn a_lost_block_is_recovered_when_the_next_one_arrives() {
        let signer = new_host_key();
        let invite = invite(signer.address());

        let mut host = host_peer(&signer, &invite);
        let mut guest = guest_peer(&invite);

        let hello = guest.session.hello(&guest.node).unwrap();
        pump(&mut [&mut host, &mut guest], 1, vec![hello]);

        let tx1 = guest.session.submit(&signed_deploy(0)).unwrap();
        pump(&mut [&mut host, &mut guest], 1, vec![tx1]);
        wait_until("block 1", || host.node.head_height() == Some(1));

        let tx2 = guest.session.submit(&signed_deploy(1)).unwrap();
        pump(&mut [&mut host, &mut guest], 1, vec![tx2]);
        wait_until("block 2", || host.node.head_height() == Some(2));

        let mut blocks = host.session.poll(&host.node).unwrap();
        assert_eq!(blocks.len(), 2);

        // Block #1 is lost on the way; #2 arrives.
        let second = blocks.pop().unwrap();
        pump(&mut [&mut host, &mut guest], 0, vec![second]);

        // The guest noticed the gap, asked again, and caught up.
        assert_eq!(guest.node.head_height(), Some(2));
    }

    #[test]
    fn blocks_not_signed_by_the_host_are_ignored() {
        let signer = new_host_key();
        let invite = invite(signer.address());

        let mut host = host_peer(&signer, &invite);
        let mut guest = guest_peer(&invite);

        // Another member of the room (same key, wrong signer) tries to impose a chain.
        let impostor = new_host_key();
        let mut fake_host = Session {
            invite: Invite {
                host: impostor.address(),
                ..invite.clone()
            },
            role: Role::Host {
                signer: impostor,
                broadcast_next: 0,
            },
            blocks_per_reply: 8,
        };

        let forged = fake_host.poll(&host.node).unwrap();
        assert_eq!(forged.len(), 1);

        let outcome = guest.session.handle(&guest.node, &forged[0]);
        assert!(outcome.applied.is_none());
        assert!(outcome.note.unwrap().contains("not signed by the host"));
        assert_eq!(guest.node.head_height(), None);

        // The real host's block is accepted afterwards.
        let hello = guest.session.hello(&guest.node).unwrap();
        pump(&mut [&mut host, &mut guest], 1, vec![hello]);
        assert_eq!(guest.node.head_height(), Some(0));
    }

    #[test]
    fn traffic_for_other_rooms_or_junk_is_ignored() {
        let signer = new_host_key();
        let invite = invite(signer.address());

        let mut host = host_peer(&signer, &invite);
        let mut guest = guest_peer(&invite);

        let stranger = Session::guest(Invite {
            key: new_room_key(),
            ..invite.clone()
        });

        let from_stranger = stranger.hello(&guest.node).unwrap();
        assert_eq!(
            host.session.handle(&host.node, &from_stranger),
            Outcome::default()
        );

        assert_eq!(host.session.handle(&host.node, b"junk"), Outcome::default());
        assert_eq!(guest.session.handle(&guest.node, &[]), Outcome::default());

        // A guest does not act on hellos or transactions (only the host does),
        // and the host does not act on blocks.
        let hello = guest.session.hello(&guest.node).unwrap();
        assert_eq!(
            guest.session.handle(&guest.node, &hello),
            Outcome::default()
        );
    }

    #[test]
    fn a_bad_guest_transaction_is_reported_not_fatal() {
        let signer = new_host_key();
        let invite = invite(signer.address());

        let mut host = host_peer(&signer, &invite);
        let guest = Session::guest(invite);

        let garbage = guest.submit(&[1, 2, 3]).unwrap();
        let outcome = host.session.handle(&host.node, &garbage);

        assert!(outcome.note.unwrap().contains("refused"));
        assert!(host.node.halted().is_none());
    }

    #[test]
    fn the_host_key_must_match_the_invite() {
        let signer = new_host_key();
        let dir = temp_dir("mismatch");
        let node = EmbeddedNode::start(EmbeddedConfig::local(&dir)).unwrap();

        let result = Session::host(invite(new_host_key().address()), signer, &node);
        assert!(result.is_err());

        drop(node);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
