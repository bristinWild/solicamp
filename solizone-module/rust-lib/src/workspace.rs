//! A room's contract files.
//!
//! Each room keeps its Solidity sources as plain `.sol` files in folders under
//! `<room data dir>/contracts`, so they can also be opened in any editor or
//! put under git. Every path the UI sends is relative to that folder and is
//! checked here: no `..`, no absolute paths, no hidden names, nothing that
//! leaves the folder (symlinks included).

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

const DIR: &str = "contracts";

/// Largest file the UI may read or write.
const MAX_FILE: usize = 1 << 20;

/// Most entries `list` returns, and deepest folder nesting it follows.
const MAX_ENTRIES: usize = 2000;
const MAX_DEPTH: usize = 8;

pub fn root(room_dir: &Path) -> PathBuf {
    room_dir.join(DIR)
}

/// Checks a relative path from the UI and returns it normalised.
pub fn clean(rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();

    if rel.is_empty() {
        return Err("give a file or folder name".to_string());
    }

    if rel.contains('\\') || rel.chars().any(char::is_control) {
        return Err("names cannot contain backslashes or control characters".to_string());
    }

    let mut out = PathBuf::new();
    let mut depth = 0;

    for part in Path::new(rel).components() {
        match part {
            Component::Normal(name) => {
                let name = name.to_str().ok_or("names must be valid text")?;

                if name.starts_with('.') || name.starts_with('-') {
                    return Err("names cannot start with '.' or '-'".to_string());
                }

                if name.len() > 100 {
                    return Err("names can be at most 100 characters".to_string());
                }

                out.push(name);
                depth += 1;
            }
            _ => return Err("paths are relative: no '..', '.' or leading '/'".to_string()),
        }
    }

    if depth == 0 || depth > MAX_DEPTH {
        return Err(format!("paths can be at most {MAX_DEPTH} folders deep"));
    }

    Ok(out)
}

/// A file path must end in `.sol`.
pub fn clean_file(rel: &str) -> Result<PathBuf, String> {
    let path = clean(rel)?;

    if path.extension().and_then(|ext| ext.to_str()) != Some("sol") {
        return Err("contract files must end in .sol".to_string());
    }

    Ok(path)
}

/// The workspace folder, created on first use.
fn ensure_root(root: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("cannot create {}: {error}", root.display()))?;

    root.canonicalize()
        .map_err(|error| format!("cannot open {}: {error}", root.display()))
}

/// Joins `rel` to the workspace and makes sure the result, or its nearest
/// existing ancestor, is still inside it once symlinks are resolved.
fn resolve(root: &Path, rel: &Path) -> Result<PathBuf, String> {
    let canonical = ensure_root(root)?;
    let path = canonical.join(rel);

    let mut probe = path.as_path();
    loop {
        if let Ok(real) = probe.canonicalize() {
            if !real.starts_with(&canonical) {
                return Err("that path leaves the contracts folder".to_string());
            }
            break;
        }

        match probe.parent() {
            Some(parent) => probe = parent,
            None => return Err("that path leaves the contracts folder".to_string()),
        }
    }

    Ok(path)
}

/// Every folder and `.sol` file, parents before their children:
/// `{"entries": [{"path", "kind": "dir"|"file"}]}`.
pub fn list(root: &Path) -> Result<Value, String> {
    let canonical = ensure_root(root)?;
    let mut entries: Vec<(Vec<String>, &'static str)> = Vec::new();

    walk(&canonical, &mut Vec::new(), &mut entries)?;

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let list: Vec<Value> = entries
        .iter()
        .map(|(parts, kind)| json!({ "path": parts.join("/"), "kind": kind }))
        .collect();

    Ok(json!({ "entries": list }))
}

fn walk(
    dir: &Path,
    prefix: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, &'static str)>,
) -> Result<(), String> {
    if prefix.len() >= MAX_DEPTH {
        return Ok(());
    }

    let read =
        fs::read_dir(dir).map_err(|error| format!("cannot read {}: {error}", dir.display()))?;

    for item in read.flatten() {
        if out.len() >= MAX_ENTRIES {
            break;
        }

        let name = match item.file_name().into_string() {
            Ok(name) if !name.starts_with('.') => name,
            _ => continue,
        };

        let kind = match item.file_type() {
            Ok(kind) if !kind.is_symlink() => kind,
            _ => continue,
        };

        let mut parts = prefix.clone();
        parts.push(name.clone());

        if kind.is_dir() {
            out.push((parts.clone(), "dir"));
            prefix.push(name);
            walk(&item.path(), prefix, out)?;
            prefix.pop();
        } else if kind.is_file() && name.ends_with(".sol") {
            out.push((parts, "file"));
        }
    }

    Ok(())
}

/// `{"path", "content"}`.
pub fn read(root: &Path, rel: &str) -> Result<Value, String> {
    let rel = clean_file(rel)?;
    let path = resolve(root, &rel)?;

    let meta = fs::metadata(&path).map_err(|_| format!("{} does not exist", rel.display()))?;

    if !meta.is_file() {
        return Err(format!("{} is not a file", rel.display()));
    }

    if meta.len() as usize > MAX_FILE {
        return Err("that file is larger than 1 MB".to_string());
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", rel.display()))?;

    Ok(json!({ "path": slash(&rel), "content": content }))
}

/// Saves `content`, replacing the file if it exists. The write goes to a
/// temporary file first, so a crash never leaves half a file.
pub fn write(root: &Path, rel: &str, content: &str) -> Result<Value, String> {
    let rel = clean_file(rel)?;

    if content.len() > MAX_FILE {
        return Err("files can be at most 1 MB".to_string());
    }

    let path = resolve(root, &rel)?;

    if path.is_dir() {
        return Err(format!("{} is a folder", rel.display()));
    }

    store(&path, content)?;

    Ok(json!({ "path": slash(&rel), "bytes": content.len() }))
}

/// Like `write`, but refuses to replace an existing file.
pub fn create(root: &Path, rel: &str, content: &str) -> Result<Value, String> {
    let checked = clean_file(rel)?;

    if resolve(root, &checked)?.exists() {
        return Err(format!("{} already exists", checked.display()));
    }

    write(root, rel, content)
}

fn store(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("bad file name")?;
    let temp = path.with_file_name(format!(".{name}.tmp"));

    fs::write(&temp, content)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;

    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot save {}: {error}", path.display())
    })
}

pub fn mkdir(root: &Path, rel: &str) -> Result<Value, String> {
    let rel = clean(rel)?;
    let path = resolve(root, &rel)?;

    if path.exists() {
        return Err(format!("{} already exists", rel.display()));
    }

    fs::create_dir_all(&path)
        .map_err(|error| format!("cannot create {}: {error}", rel.display()))?;

    Ok(json!({ "path": slash(&rel) }))
}

/// Renames or moves a file or folder. Files keep the `.sol` ending.
pub fn rename(root: &Path, from: &str, to: &str) -> Result<Value, String> {
    let from = clean(from)?;
    let from_path = resolve(root, &from)?;

    let to = if from_path.is_dir() {
        clean(to.trim())?
    } else {
        clean_file(to)?
    };

    if !from_path.exists() {
        return Err(format!("{} does not exist", from.display()));
    }

    if to.starts_with(&from) && to != from {
        return Err("a folder cannot be moved into itself".to_string());
    }

    let to_path = resolve(root, &to)?;

    if to_path.exists() {
        return Err(format!("{} already exists", to.display()));
    }

    if let Some(parent) = to_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }

    fs::rename(&from_path, &to_path)
        .map_err(|error| format!("cannot move {}: {error}", from.display()))?;

    Ok(json!({ "from": slash(&from), "to": slash(&to) }))
}

/// Deletes a file, or a folder with everything in it.
pub fn delete(root: &Path, rel: &str) -> Result<Value, String> {
    let rel = clean(rel)?;
    let path = resolve(root, &rel)?;

    let result = if path.is_dir() {
        fs::remove_dir_all(&path)
    } else if path.is_file() {
        fs::remove_file(&path)
    } else {
        return Err(format!("{} does not exist", rel.display()));
    };

    result.map_err(|error| format!("cannot delete {}: {error}", rel.display()))?;

    Ok(json!({ "deleted": slash(&rel) }))
}

/// The absolute path of an existing file in the workspace, for the compiler.
pub fn existing_file(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = clean_file(rel)?;

    if !resolve(root, &rel)?.is_file() {
        return Err(format!("{} does not exist", rel.display()));
    }

    Ok(rel)
}

fn slash(path: &Path) -> String {
    path.components()
        .filter_map(|part| part.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solicamp-ws-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn paths(root: &Path) -> Vec<String> {
        list(root).unwrap()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                format!(
                    "{}:{}",
                    e["kind"].as_str().unwrap(),
                    e["path"].as_str().unwrap()
                )
            })
            .collect()
    }

    #[test]
    fn rejects_paths_that_escape_or_hide() {
        for bad in [
            "",
            "  ",
            "/etc/passwd",
            "../x.sol",
            "a/../../x.sol",
            "./x.sol",
            ".hidden.sol",
            "-x.sol",
            "a\\b.sol",
            "a\u{0}b.sol",
        ] {
            assert!(clean(bad).is_err(), "{bad:?} should be refused");
        }

        assert_eq!(clean("a/b/C.sol").unwrap(), PathBuf::from("a/b/C.sol"));
        assert!(clean(&"d/".repeat(20)).is_err());
        assert!(clean_file("notes.txt").is_err());
        assert!(clean_file("folder").is_err());
    }

    #[test]
    fn files_and_folders_round_trip() {
        let root = temp("flow").join("contracts");

        assert!(paths(&root).is_empty());

        create(&root, "Counter.sol", "contract Counter {}").unwrap();
        mkdir(&root, "tokens").unwrap();
        create(&root, "tokens/Coin.sol", "contract Coin {}").unwrap();
        mkdir(&root, "tokens/old").unwrap();

        assert_eq!(
            paths(&root),
            [
                "file:Counter.sol",
                "dir:tokens",
                "file:tokens/Coin.sol",
                "dir:tokens/old"
            ]
        );

        assert!(create(&root, "Counter.sol", "x")
            .unwrap_err()
            .contains("already exists"));
        assert_eq!(
            read(&root, "tokens/Coin.sol").unwrap()["content"],
            "contract Coin {}"
        );

        write(&root, "Counter.sol", "contract Counter { uint x; }").unwrap();
        assert_eq!(
            read(&root, "Counter.sol").unwrap()["content"],
            "contract Counter { uint x; }"
        );

        // Moving into a folder, renaming, and the folder-into-itself guard.
        rename(&root, "Counter.sol", "tokens/Counter.sol").unwrap();
        rename(&root, "tokens/Coin.sol", "tokens/Token.sol").unwrap();
        assert!(rename(&root, "tokens", "tokens/old/tokens")
            .unwrap_err()
            .contains("itself"));
        assert!(rename(&root, "tokens/Token.sol", "tokens/Token.txt").is_err());
        rename(&root, "tokens", "assets").unwrap();

        assert_eq!(
            paths(&root),
            [
                "dir:assets",
                "file:assets/Counter.sol",
                "file:assets/Token.sol",
                "dir:assets/old"
            ]
        );

        delete(&root, "assets/old").unwrap();
        delete(&root, "assets").unwrap();
        assert!(paths(&root).is_empty());
        assert!(delete(&root, "assets").is_err());

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn lists_siblings_in_order_and_skips_other_files() {
        let root = temp("order").join("contracts");

        create(&root, "a/x.sol", "").unwrap();
        create(&root, "a-b/y.sol", "").unwrap();
        create(&root, "b.sol", "").unwrap();
        fs::write(root.join("notes.txt"), "no").unwrap();
        fs::write(root.join(".hidden.sol"), "no").unwrap();

        assert_eq!(
            paths(&root),
            [
                "dir:a",
                "file:a/x.sol",
                "dir:a-b",
                "file:a-b/y.sol",
                "file:b.sol"
            ]
        );

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_reach_outside() {
        let base = temp("link");
        let root = base.join("contracts");
        let outside = base.join("outside");

        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.sol"), "secret").unwrap();
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

        assert!(read(&root, "escape/secret.sol").is_err());
        assert!(write(&root, "escape/new.sol", "x").is_err());
        assert!(paths(&root).is_empty());

        let _ = fs::remove_dir_all(&base);
    }
}
