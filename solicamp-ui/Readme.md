# solicamp-ui

**A Solidity playground that feels native to Logos Basecamp.**

`solicamp-ui` is the front end of SoliCamp: a Remix-style workspace written in native Qt QML (no web view) and loaded by Basecamp as a `ui_qml` module. It talks to [`solizone-module`](../solizone-module), which runs the chain and compiler.

`Qt QML` · `Logos Basecamp ui_qml` · `Solidity` · `Solizone EVM`

> Independent project. Not an official Logos project.

---

## What you can do

1. **Write Solidity** in a large editor, with several files and folders per project.
2. **Compile** with one click, with imports between files.
3. **Deploy** to a local chain and **call** any function from an auto-generated Interact panel.
4. **Work in rooms.** Each room is its own chain with its own files.
5. **Share a room** with a teammate through an invite. They verify every block on their own machine.
6. **Publish to Bedrock** when you want it on the network. It is off by default.

Like Anvil, it is a sandbox first. **Nothing is published and no fees are charged until you press Publish.**

## Layout

```text
┌ top bar: files · rooms toggles        interact · publish toggles   chain · signer ┐
├───────────┬───────────┬─────────────────────────────┬──────────────────┤
│  Rooms    │  Files    │                             │  Interact        │
│  (drawer) │  (drawer) │        Code editor          │  Publish         │
│           │           │       (the big part)        │  (drawer)        │
├───────────┴───────────┴─────────────────────────────┴──────────────────┤
│ Output (collapsible with the arrow next to its title)                    │
└──────────────────────────────────────────────────────────────────────────┘
```

The editor gets the space. Rooms, files, interact and publish are drawers you open only when you need them. Panes are resizable, and the output panel folds away.

## Highlights

- **File explorer.** Create, rename and delete files and folders. Drag and drop to move. Files autosave, and `Ctrl+S` saves now.
- **Interact panel.** Functions are read from the compiled ABI. Reads run free, writes send a transaction, and results and events are decoded for you.
- **Publish panel.** Shows state (off, needs funding, syncing, published), the paying wallet address and its balance, the list of wallet addresses, the channel, and block progress. You can pick which wallet address pays the fees.
- **Rounded buttons, square panels.** A deliberate look, with dark cypherpunk colours and clear states.
- **Narrow-pane friendly.** The layout adapts to small Basecamp panels.

## How it talks to the core

The UI has no chain logic. It calls the core module through the Basecamp bridge:

```qml
logos.callModuleAsync("solizone_module", "compile_file", [path], function (result) { … })
```

Every reply is JSON text, and failures are `{"error": "…"}`. See the [method list](../solizone-module#the-interface).

## Build (macOS, Nix)

Point the `solizone_module` input in `flake.nix` at your `solizone-module` checkout, then:

```sh
git add -A                      # flakes only see tracked files
nix build .#lgx-portable
```

Install both `.lgx` packages in Basecamp's Package Manager (uninstall the old ones first when updating), and open **SoliCamp**.

## Notes for contributors

- Basecamp's QML engine rejects JavaScript reserved and future-reserved words used as names (`short`, `long`, `int`, `char`, `byte`, `final`, `native`, `static`, `public`, `private`, `package`, `interface`, `enum`, `export`, …) even when a desktop Qt accepts them. Avoid them.
- `SplitView` handles write `preferredWidth` and `preferredHeight` when dragged, which breaks bindings. Toggle panes imperatively.
- Keep layouts tolerant of narrow panes.

## Status

Working in Basecamp. Next: syntax highlighting in the editor and a live two-window collaboration test.

## License

MIT, see the [LICENSE](../LICENSE) at the repo root.