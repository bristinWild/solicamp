//! Finding and running the Solidity compiler.
//!
//! The module compiles with a `solc` binary. Lookup order:
//!
//! 1. `$SOLIZONE_SOLC`, an explicit path;
//! 2. `solc` on `PATH` (the Nix package puts one in the runtime closure);
//! 3. Foundry's compiler cache, `~/.svm/<version>/solc-<version>`, newest first.
//!
//! The last step is a development convenience. Shipping a pinned `solc` inside
//! the package is the long-term answer; this is what the Basecamp spike checks.

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{json, Value};

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

    if let Some(found) = on_path("solc") {
        return Ok(found);
    }

    if let Some(found) = newest_in_svm() {
        return Ok(found);
    }

    Err(
        "no Solidity compiler found: set SOLIZONE_SOLC, put solc on PATH, or run \
         `forge build` once so Foundry caches one in ~/.svm"
            .to_string(),
    )
}

fn on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// `~/.svm/<version>/solc-<version>`, highest version first.
fn newest_in_svm() -> Option<PathBuf> {
    let svm = PathBuf::from(env::var_os("HOME")?).join(".svm");

    let mut versions: Vec<((u64, u64, u64), PathBuf)> = fs::read_dir(svm)
        .ok()?
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
