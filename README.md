<div align="center">

# SoliCamp

**Remix for Logos.** Write, compile, deploy and test Solidity inside Logos Basecamp, then publish to Bedrock when you are ready.

</div>

> Independent project. Not an official Logos project.

| Folder | What it is |
|---|---|
| [`solizone-module/`](./solizone-module) | Rust core module: embeds a [Solizone EVM](https://github.com/bristinWild/solizone) node, compiles Solidity, manages rooms, files, collaboration and publishing |
| [`solicamp-ui/`](./solicamp-ui) | Native Qt QML front end loaded by Basecamp |

## Why

Logos needs builders, and most builders already know Solidity. SoliCamp gives them a familiar playground *inside* Basecamp, with no server and nothing to install except two packages. It is a local, free sandbox like Anvil. Publishing blocks to Logos is a deliberate, opt-in button.

## What makes it different

- **Native.** A Rust module and a native QML UI, with no web view and no hosted backend.
- **Private by design.** Your chain and files stay on your machine. Shared rooms are sealed with a key that only travels inside the invite.
- **Trustless sharing.** Guests re-execute every block the host produces, so the host cannot fake state.
- **Opt-in publishing.** Each room has its own channel and key, funded from your Basecamp wallet.
- **Part of a stack.** It runs on Solizone EVM and pairs with [Zoner](https://github.com/bristinWild/Zoner), the Foundry companion, for command-line developers.

## Quick start

```sh
# in each folder
git add -A && nix build .#lgx-portable
```

Install both packages in Basecamp's Package Manager and open SoliCamp. Details are in each folder's README.

## Status

Early and working. Compile, deploy, call, multi-file workspaces, rooms and optional publishing run in Basecamp. Next: a live two-window collaboration test, editor syntax highlighting, sequencer-key backup and per-guest approval.

## License

MIT, see [LICENSE](./LICENSE). The vendored Logos Rust SDK copy in `solizone-module/rust-lib/vendor/logos-rust-sdk-compat` keeps its own MIT / Apache-2.0 licenses.