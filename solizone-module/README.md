# solizone-module

**A full Solidity chain inside a Logos Basecamp module.** No server, no open port, no child process.

`solizone-module` is a Logos *core module* written in Rust. It embeds a [Solizone EVM](https://github.com/bristinWild/solizone) node (REVM) directly in Basecamp, so other modules, including the [SoliCamp UI](../solicamp-ui), can compile, deploy and call Solidity contracts through plain method calls.

`Rust` · `REVM` · `Logos Core Module` · `Logos Zone SDK` · `Logos Messaging`

> Independent project. Not an official Logos project.

---

## What it does

| Capability | Details |
|---|---|
| **Local EVM chain** | Starts or recovers a Solizone node in a data directory (default `~/.solicamp`). Pre-funded dev accounts, instant blocks, state that survives restarts. |
| **Solidity compiler** | Compiles with a pinned `solc`. Supports multi-file projects with imports (`compile_file`). |
| **Deploy and call** | Deploy, send transactions, read calls, ABI encode and decode, receipts, raw JSON-RPC. |
| **Rooms** | Separate local chains, each with its own chain id (a transaction signed for one room is invalid in every other), its own data and history. |
| **Workspace** | Plain `.sol` files and folders for each room, with strict path checks. |
| **Collaboration** | Share a room by invite. Guests replay and verify the host's blocks over Logos Messaging. |
| **Publish to Bedrock** | Optional. Publishes a room's blocks to Logos through the Zone SDK, paid from the Basecamp wallet. Off until you turn it on. |

## Design choices

- **Sandbox first.** The chain is local and free, like Anvil. Nothing is published and no fees are charged until the user presses *Publish*.
- **No reset button, on purpose.** History in a room only grows. To start over, create a new room. This keeps chains honest and discourages spam and misuse.
- **Nobody has to trust anybody.** In a shared room, a guest executes every block itself, so a host cannot fake state. Blocks are signed by the host key named in the invite. All traffic is sealed with a key that only travels inside the invite, because the messaging network is public.
- **Per-room publishing identity.** Each room gets its own channel and sequencer key, generated on first use. Only a shared room's host publishes.
- **Safe file handling.** Workspace paths reject `..`, absolute paths, hidden names, backslashes and control characters, limit depth and size, guard against symlink escapes, and write atomically.
- **Pinned compiler.** `solc` is a pinned release, checked against a SHA-256 compiled into the module. `$SOLIZONE_SOLC` overrides it.

## The interface

The Rust trait `SolizoneModule` in `rust-lib/src/lib.rs` is the contract. Logos' module builder derives the `.lidl` interface from it. Every method returns JSON text, and failures come back as `{"error": "..."}`.

| Group | Methods |
|---|---|
| Lifecycle | `start(dataDir)` · `stop` · `status` · `accounts` |
| Rooms | `list_rooms` · `create_room` · `open_room` · `share_room` · `join_room` |
| Compile | `compile(source)` · `compile_file(path)` |
| Files | `fs_list` · `fs_read` · `fs_write` · `fs_create` · `fs_mkdir` · `fs_rename` · `fs_delete` |
| Chain | `deploy` · `send` · `read` · `receipt` · `encode` · `decode` · `rpc` |
| Collaboration | `net_start` · `net_stop` · `net_status` · `collab_hello` · `collab_poll` · `collab_incoming` |
| Publishing | `publish_status` · `publish_start` · `publish_stop` · `publish_use_address` · `publish_import` |

`start("")` uses `~/.solicamp`.

## Source map

```text
rust-lib/src/
├── lib.rs        # the module contract (trait) and thin wrappers
├── engine.rs     # chain logic: start, rooms, deploy, send, compile
├── solc.rs       # finds, verifies and runs the Solidity compiler
├── workspace.rs  # per-room .sol files and folders
├── rooms.rs      # separate chains with their own ids
├── collab.rs     # host/guest protocol (no networking, easy to test)
├── net.rs        # carries collab messages over Logos Messaging
├── publish.rs    # Publish to Bedrock: wallet, funding, status
└── vendor/       # Solizone EVM node and a patched Logos Rust SDK
```

## Build (macOS, Nix)

```sh
# 1. the Rust SDK sources the module links against
nix build github:logos-co/logos-module-builder/0.3.2#rust-sdk-src -o logos-rust-sdk-src

# 2. vendor the node
scripts/sync-solizone-evm.sh ~/logos/solizone/solizone-evm

# 3. lockfile (committed; Nix builds offline from it)
(cd rust-lib && cargo generate-lockfile)

# 4. flakes only see tracked files
git add -A

nix build .#lgx-portable      # package for Basecamp
```

Install the resulting `.lgx` in Basecamp's Package Manager. Install `solicamp-ui` too.

## Tests

```sh
cd rust-lib && cargo test --lib
```

The unit tests cover the workspace and path checks, solc output parsing, the collaboration protocol (including an in-memory two-person room), the publishing state machine and funding choice.

## Status

Early but working: compile, deploy, call, files, rooms and the optional publisher run inside Basecamp. Still to do: a live two-window collaboration test, a first-publish confirmation step, sequencer-key backup and per-guest approval.

## License

MIT, see the [LICENSE](../LICENSE) at the repo root. The vendored Logos Rust SDK copy in `rust-lib/vendor/logos-rust-sdk-compat` keeps its own MIT / Apache-2.0 licenses.