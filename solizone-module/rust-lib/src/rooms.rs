//! Rooms: separate local chains inside one SoliCamp installation.
//!
//! A room is a fork in the sense that matters for a sandbox: it has its own
//! chain id (so a transaction signed for one room is invalid in every other),
//! its own data directory and its own history. The built-in `main` room is the
//! canonical Solizone dev chain (id 9001) and lives directly in the base
//! directory, exactly where earlier versions kept it.
//!
//! There is deliberately no "reset": history in a room only grows. To start
//! over, create a new room.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const MAIN_ID: &str = "main";
pub const MAIN_CHAIN_ID: u64 = 9001;

/// Fork chain ids live in `0x5A000000..=0x5AFFFFFF` (1.5 billion range, far
/// from any real network and below 2^53 so wallets can represent them).
const FORK_BASE: u64 = 0x5A00_0000;
const MAX_NAME: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub chain_id: u64,
    pub created: u64,
}

impl Room {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "chainId": self.chain_id,
            "created": self.created,
        })
    }
}

pub struct Rooms {
    base: PathBuf,
}

impl Rooms {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    fn registry(&self) -> PathBuf {
        self.base.join("rooms.json")
    }

    fn main_room() -> Room {
        Room {
            id: MAIN_ID.to_string(),
            name: "Main".to_string(),
            chain_id: MAIN_CHAIN_ID,
            created: 0,
        }
    }

    /// Where a room keeps its data. `main` keeps the original location.
    pub fn data_dir(&self, room: &Room) -> PathBuf {
        if room.id == MAIN_ID {
            self.base.clone()
        } else {
            self.base.join("rooms").join(&room.id)
        }
    }

    /// `main` first, then the others in creation order.
    pub fn list(&self) -> Result<Vec<Room>, String> {
        let mut rooms = vec![Self::main_room()];
        rooms.extend(self.stored()?);

        Ok(rooms)
    }

    pub fn get(&self, id: &str) -> Result<Room, String> {
        self.list()?
            .into_iter()
            .find(|room| room.id == id)
            .ok_or_else(|| format!("no room with id {id:?}"))
    }

    pub fn create(&self, name: &str) -> Result<Room, String> {
        let name = clean_name(name)?;
        let existing = self.list()?;
        let created = unix_seconds();

        // Derive the id and chain id from the name, time and a counter; retry
        // on the (unlikely) collision with an existing room.
        let mut attempt = 0u32;

        let room = loop {
            let seed = Sha256::digest(
                format!(
                    "{name}|{created}|{}|{attempt}|{:?}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.subsec_nanos())
                        .unwrap_or(0)
                )
                .as_bytes(),
            );

            let id = hex::encode(&seed[..4]);
            let chain_id = FORK_BASE | (u32::from_be_bytes([0, seed[4], seed[5], seed[6]]) as u64);

            let taken = existing
                .iter()
                .any(|room| room.id == id || room.chain_id == chain_id);

            if !taken {
                break Room {
                    id,
                    name: name.clone(),
                    chain_id,
                    created,
                };
            }

            attempt += 1;
        };

        let mut stored = self.stored()?;
        stored.push(room.clone());
        self.save(&stored)?;

        Ok(room)
    }

    fn stored(&self) -> Result<Vec<Room>, String> {
        let path = self.registry();

        if !path.exists() {
            return Ok(Vec::new());
        }

        let text = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let list: Vec<Value> = serde_json::from_str(&text)
            .map_err(|error| format!("{} is corrupt: {error}", path.display()))?;

        list.iter()
            .map(|entry| {
                Ok(Room {
                    id: entry["id"].as_str().ok_or("room without id")?.to_string(),
                    name: entry["name"].as_str().unwrap_or("").to_string(),
                    chain_id: entry["chainId"].as_u64().ok_or("room without chainId")?,
                    created: entry["created"].as_u64().unwrap_or(0),
                })
            })
            .collect()
    }

    fn save(&self, rooms: &[Room]) -> Result<(), String> {
        fs::create_dir_all(&self.base).map_err(|error| error.to_string())?;

        let body = Value::Array(rooms.iter().map(Room::to_json).collect());
        let temporary = self.registry().with_extension("json.tmp");

        fs::write(&temporary, body.to_string()).map_err(|error| error.to_string())?;
        fs::rename(&temporary, self.registry()).map_err(|error| error.to_string())
    }

    pub fn base(&self) -> &Path {
        &self.base
    }
}

fn clean_name(name: &str) -> Result<String, String> {
    let name: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();

    if name.is_empty() {
        Err("a room needs a name".to_string())
    } else {
        Ok(name)
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rooms-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn main_is_always_there_and_first() {
        let dir = temp("main");
        let rooms = Rooms::new(&dir);

        let list = rooms.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, MAIN_ID);
        assert_eq!(list[0].chain_id, 9001);
        assert_eq!(rooms.data_dir(&list[0]), dir);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn created_rooms_get_distinct_ids_chain_ids_and_folders() {
        let dir = temp("create");
        let rooms = Rooms::new(&dir);

        let a = rooms.create("  Hackathon  ").unwrap();
        let b = rooms.create("Hackathon").unwrap();

        assert_eq!(a.name, "Hackathon");
        assert_ne!(a.id, b.id);
        assert_ne!(a.chain_id, b.chain_id);

        for room in [&a, &b] {
            assert!((0x5A00_0000..=0x5AFF_FFFF).contains(&room.chain_id));
            assert_ne!(room.chain_id, MAIN_CHAIN_ID);
            assert_eq!(rooms.data_dir(room), dir.join("rooms").join(&room.id));
        }

        // Persisted, in creation order, after main.
        let ids: Vec<_> = Rooms::new(&dir)
            .list()
            .unwrap()
            .into_iter()
            .map(|room| room.id)
            .collect();
        assert_eq!(ids, vec![MAIN_ID.to_string(), a.id.clone(), b.id.clone()]);

        assert_eq!(rooms.get(&b.id).unwrap(), b);
        assert!(rooms.get("nope").is_err());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_are_validated_and_truncated() {
        let dir = temp("names");
        let rooms = Rooms::new(&dir);

        assert!(rooms.create("   ").is_err());
        assert!(rooms.create("\n\t").is_err());
        assert_eq!(rooms.create(&"x".repeat(200)).unwrap().name.len(), MAX_NAME);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_registry_is_an_error_not_a_silent_reset() {
        let dir = temp("corrupt");
        fs::write(dir.join("rooms.json"), "not json").unwrap();

        assert!(Rooms::new(&dir).list().unwrap_err().contains("corrupt"));

        fs::remove_dir_all(&dir).unwrap();
    }
}
