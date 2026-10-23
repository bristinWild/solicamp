# solizone-module

A Logos **core module** (Rust) that runs a Solizone EVM node inside Basecamp.
`solicamp-ui` (QML) calls it to compile, deploy and call Solidity contracts.

Methods (all return JSON text; failures are `{"error": "..."}`):
`start(dataDir)` · `stop` · `status` · `accounts` · `compile(source)` ·
`deploy(account, bytecode, ctorArgs)` · `send(account, to, data, valueWei)` ·
`encode(abi, fn, argsJson)` · `decode(abi, fn, data)` · `read(to, data)` · `rpc(json)`.

`start("")` uses `~/.solicamp`.

## Build (Mac, Nix)

```sh
# 1. the Rust SDK sources the module links against
nix build github:logos-co/logos-module-builder/0.3.2#rust-sdk-src -o logos-rust-sdk-src

# 2. vendor the node (needs the solizone-evm `logos` feature commit)
scripts/sync-solizone-evm.sh ~/logos/solizone/solizone-evm

# 3. lockfile (committed; Nix builds offline from it)
(cd rust-lib && cargo generate-lockfile)

# 4. flakes only see tracked files
git add -A

nix build .#lgx      # package for Basecamp
```