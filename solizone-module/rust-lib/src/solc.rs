//! Finding and running the Solidity compiler.
//!
//! The module compiles with a `solc` binary. Lookup order:
//!
//! 1. `$SOLIZONE_SOLC`, an explicit path;
//! 2. `solc` on `PATH` (the Nix package puts one in the runtime closure);
//! 3. Foundry's compiler cache (`~/.svm`, `~/Library/Application Support/svm`
//!    on macOS, `~/.local/share/svm` on Linux), newest version first.
//!
//! The last step is a development convenience. Shipping a pinned `solc` inside
//! the package is the long-term answer; this is what the Basecamp spike checks.

//! Finding and running the Solidity compiler.
//!
//! The module compiles with a pinned `solc` release. Lookup order:
//!
//! 1. `$SOLIZONE_SOLC`, an explicit path;
//! 2. the pinned release in `~/.solicamp/solc/<version>/solc`, downloaded once
//!    from the official GitHub release and checked against a SHA-256 that is
//!    compiled into this module;
//! 3. if that download is impossible (offline): `solc` on `PATH`, then
//!    Foundry's compiler cache (`~/.svm`, `~/Library/Application Support/svm`
//!    on macOS, `~/.local/share/svm` on Linux), newest version first.

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The compiler release this module is built and tested against.
pub const PINNED_VERSION: &str = "0.8.28";

struct Release {
    asset: &'static str,
    sha256: &'static str,
}

/// The official release asset for this platform, if there is one.
fn release() -> Option<Release> {
    if cfg!(target_os = "macos") {
        // Universal binary (arm64 + x86_64).
        Some(Release {
            asset: "solc-macos",
            sha256: "81515b0e53deaa266d549545ccaac0a5a96e6d4e8201c77f673b2c710976d9ea",
        })
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(Release {
            asset: "solc-static-linux",
            sha256: "9a0fb7e0db2c0641dbae1c5cc645dc686820c83af516226abb1c0a2f76636f25",
        })
    } else {
        None
    }
}

fn pinned_path() -> Option<PathBuf> {
    Some(
        PathBuf::from(env::var_os("HOME")?)
            .join(".solicamp/solc")
            .join(PINNED_VERSION)
            .join("solc"),
    )
}

fn sha256_hex(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();

    std::io::copy(&mut file, &mut hasher).map_err(|error| error.to_string())?;

    Ok(hex::encode(hasher.finalize()))
}

/// Downloads the pinned compiler (once) and returns its path.
fn ensure_pinned() -> Result<PathBuf, String> {
    let target = pinned_path().ok_or("HOME is not set")?;

    if target.is_file() {
        return Ok(target);
    }

    let release = release().ok_or("no pinned solc for this platform")?;
    let dir = target.parent().ok_or("bad path")?;

    fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;

    let partial = dir.join("solc.part");
    let url = format!(
        "https://github.com/argotorg/solidity/releases/download/v{PINNED_VERSION}/{}",
        release.asset
    );

    let status = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--tlsv1.2", "--max-time", "900"])
        .arg("--output")
        .arg(&partial)
        .arg(&url)
        .status()
        .map_err(|error| format!("cannot run curl to download solc: {error}"))?;

    if !status.success() {
        let _ = fs::remove_file(&partial);
        return Err(format!("downloading {url} failed ({status})"));
    }

    let actual = sha256_hex(&partial)?;

    if actual != release.sha256 {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "downloaded solc does not match the pinned checksum (expected {}, got {actual})",
            release.sha256
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&partial, fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
    }

    fs::rename(&partial, &target).map_err(|error| error.to_string())?;

    Ok(target)
}

pub fn find_solc() -> Result<PathBuf, String> {
    if let Some(explicit) = env::var_os("SOLIZONE_SOLC") {
        let path = PathBuf::from(explicit);

        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "SOLIZONE_SOLC points to {}, which is not a file",
                path.display()
            ))
        };
    }

    let download_error = match ensure_pinned() {
        Ok(path) => return Ok(path),
        Err(error) => error,
    };

    if let Some(found) = on_path("solc") {
        return Ok(found);
    }

    if let Some(found) = newest_in_svm() {
        return Ok(found);
    }

    Err(format!(
        "no Solidity compiler available: {download_error}. Connect to the internet once so \
         the pinned solc {PINNED_VERSION} can be downloaded, or set SOLIZONE_SOLC to a solc binary"
    ))
}

fn on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Folders where Foundry / svm keep downloaded compilers.
fn svm_roots() -> Vec<PathBuf> {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };

    vec![
        home.join(".svm"),
        home.join("Library/Application Support/svm"),
        home.join(".local/share/svm"),
    ]
}

/// `<root>/<version>/solc-<version>` across all roots, highest version first.
fn newest_in_svm() -> Option<PathBuf> {
    newest_in(&svm_roots())
}

fn newest_in(roots: &[PathBuf]) -> Option<PathBuf> {
    let mut versions: Vec<((u64, u64, u64), PathBuf)> = roots
        .iter()
        .filter_map(|root| fs::read_dir(root).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let key = parse_version(&name)?;
            let binary = entry.path().join(format!("solc-{name}"));

            binary.is_file().then_some((key, binary))
        })
        .collect();

    versions.sort();
    versions.pop().map(|(_, path)| path)
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');

    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

/// Compiles one Solidity source. Returns
/// `{"solc": path, "contracts": [{"name", "abi", "bytecode"}]}`; contracts with
/// no bytecode (interfaces, abstract contracts) are left out.
pub fn compile(solc: &Path, source: &str) -> Result<Value, String> {
    let mut child = Command::new(solc)
        .args(["--combined-json", "abi,bin", "--optimize", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start {}: {error}", solc.display()))?;

    child
        .stdin
        .take()
        .ok_or("no stdin for solc")?
        .write_all(source.as_bytes())
        .map_err(|error| format!("failed to send source to solc: {error}"))?;

    let output = child
        .wait_with_output()
        .map_err(|error| format!("solc did not finish: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    parse_combined_json(solc, &output.stdout)
}

fn parse_combined_json(solc: &Path, stdout: &[u8]) -> Result<Value, String> {
    let parsed: Value = serde_json::from_slice(stdout)
        .map_err(|error| format!("unreadable solc output: {error}"))?;

    let contracts = parsed["contracts"]
        .as_object()
        .ok_or("solc output has no contracts")?;

    let mut out = Vec::new();

    for (key, value) in contracts {
        let bin = value["bin"].as_str().unwrap_or("");

        if bin.is_empty() {
            continue;
        }

        // Older compilers print the ABI as a JSON string, newer ones as an array.
        let abi = match &value["abi"] {
            Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
            other => other.clone(),
        };

        out.push(json!({
            "name": key.rsplit(':').next().unwrap_or(key),
            "abi": abi,
            "bytecode": format!("0x{bin}"),
        }));
    }

    Ok(json!({ "solc": solc.display().to_string(), "contracts": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Needs network: SOLIZONE_TEST_DOWNLOAD=1 cargo test downloads_the_pinned -- --nocapture
    #[test]
    fn downloads_the_pinned_compiler_when_asked() {
        if std::env::var_os("SOLIZONE_TEST_DOWNLOAD").is_none() {
            return;
        }

        let path = ensure_pinned().expect("download");
        let out = Command::new(&path).arg("--version").output().unwrap();

        assert!(String::from_utf8_lossy(&out.stdout).contains(PINNED_VERSION));
        eprintln!("pinned solc at {}", path.display());
    }

    #[test]
    fn checksums_are_computed_over_the_whole_file() {
        let path = std::env::temp_dir().join(format!("solc-sha-{}", std::process::id()));
        fs::write(&path, "abc").unwrap();

        assert_eq!(
            sha256_hex(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        fs::remove_file(&path).unwrap();
        assert!(sha256_hex(&path).is_err());
    }

    #[test]
    fn this_platform_has_a_pinned_release_with_a_valid_checksum() {
        if let Some(release) = release() {
            assert_eq!(release.sha256.len(), 64);
            assert!(release.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn finds_the_newest_compiler_across_cache_folders() {
        let base = std::env::temp_dir().join(format!("solc-roots-{}", std::process::id()));
        let (a, b) = (base.join("a"), base.join("b"));

        for (root, version) in [(&a, "0.8.9"), (&b, "0.8.28"), (&b, "0.8.4")] {
            let dir = root.join(version);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("solc-{version}")), "").unwrap();
        }

        let found = newest_in(&[a, b, base.join("missing")]).unwrap();
        assert!(found.ends_with("0.8.28/solc-0.8.28"), "{found:?}");

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn versions_sort_numerically_not_alphabetically() {
        assert!(parse_version("0.8.9") < parse_version("0.8.20"));
        assert_eq!(parse_version("0.8.20"), Some((0, 8, 20)));
        assert_eq!(parse_version("nightly"), None);
    }

    #[test]
    fn combined_json_keeps_contracts_with_bytecode_only() {
        let stdout = br#"{"contracts":{
            "<stdin>:Counter":{"abi":[{"type":"function","name":"increment"}],"bin":"6080"},
            "<stdin>:IThing":{"abi":[],"bin":""},
            "<stdin>:Old":{"abi":"[{\"type\":\"constructor\"}]","bin":"60"}
        },"version":"0.8.20"}"#;

        let out = parse_combined_json(Path::new("/x/solc"), stdout).unwrap();
        let contracts = out["contracts"].as_array().unwrap();

        assert_eq!(contracts.len(), 2);
        assert_eq!(contracts[0]["name"], "Counter");
        assert_eq!(contracts[0]["bytecode"], "0x6080");
        assert_eq!(contracts[0]["abi"][0]["name"], "increment");
        // A string ABI from an older compiler is parsed into an array.
        assert_eq!(contracts[1]["abi"][0]["type"], "constructor");
    }

    #[test]
    fn garbage_output_is_an_error() {
        assert!(parse_combined_json(Path::new("solc"), b"nope").is_err());
        assert!(parse_combined_json(Path::new("solc"), b"{}").is_err());
    }

    // Runs only where a compiler exists (SOLIZONE_SOLC, PATH or ~/.svm).
    #[test]
    fn compiles_a_real_contract_when_a_compiler_is_available() {
        let Ok(solc) = find_solc() else {
            eprintln!("skipped: no solc available");
            return;
        };

        let result = compile(
            &solc,
            "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.20;\n\
             contract Box { uint256 public value; function set(uint256 v) public { value = v; } }",
        )
        .unwrap();

        assert_eq!(result["contracts"][0]["name"], "Box");
        assert!(result["contracts"][0]["bytecode"].as_str().unwrap().len() > 10);

        let broken = compile(&solc, "contract {").unwrap_err();
        assert!(!broken.is_empty());
    }
}
