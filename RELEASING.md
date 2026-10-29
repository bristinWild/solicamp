# Releasing SoliCamp to the Logos module catalog

The catalog is [`logos-co/logos-modules-release-base`](https://github.com/logos-co/logos-modules-release-base), a GitHub template. You fork it, add each module as a git submodule, and its GitHub Actions build the `.lgx` packages for every platform, publish a `<module>-v<version>` release and keep an `index` release up to date.

Builds run on GitHub's runners, not on your machine:

| Variant | Runner | Output |
|---|---|---|
| `darwin-arm64` | `macos-latest` | Mac (Apple Silicon) |
| `linux-amd64` | `ubuntu-latest` | Linux x86-64 |
| `linux-arm64` | `ubuntu-24.04-arm` | Linux ARM |
| `windows-x86_64` | `ubuntu-latest` (cross-compile) | only if the flake has `packages.x86_64-windows`. Our module does not, so remove this variant (step 5) |

Our modules: `solizone_module` (core, Rust) and `solicamp_ui` (QML, depends on `solizone_module`). Both are `0.1.0` in `metadata.json`.

## 0. Before you start (once)

1. Push the latest files to `bristinWild/solicamp` (README files, `LICENSE`, and `solicamp-ui/flake.nix` from this delivery). The UI flake now points to `github:bristinWild/solicamp?dir=solizone-module`, because there is no separate `solizone-module` repo.
2. Make sure a clean checkout builds. Nix only sees git-tracked files, so run `git status` and check that `solizone-module/rust-lib/Cargo.lock`, `solizone-module/rust-lib/vendor/` and `solicamp-ui/src/icons/icon.png` are tracked. `logos-rust-sdk-src` is a local symlink and is git-ignored on purpose. CI must stage it itself (see risks).
3. Test on your Mac first: `nix build .#lgx-portable` in each folder, as you already do.

## 1. Fork the catalog

On GitHub, fork `logos-co/logos-modules-release-base` to `bristinWild/logos-modules-release-base` (or a name you like). Then:

```sh
git clone --recurse-submodules https://github.com/bristinWild/logos-modules-release-base
cd logos-modules-release-base
```

In the fork: **Settings > Actions > General > Workflow permissions**, allow read and write. (The workflows also declare `contents: write`.)

## 2. Edit `logos-repo.json`

Replace every `CHANGE-ME`. Keep the template's `schemaVersion`. Suggested values:

| Key | Value |
|---|---|
| `name` | `solizone` (lowercase, no spaces) |
| `displayName` | `Solizone` |
| `description` | `Solizone EVM and SoliCamp: write, test and publish Solidity on Logos from Basecamp.` |
| `homepage` | `https://github.com/bristinWild/solicamp` |
| `indexUrl` | the URL of your fork's `index` release `index.json` (the template comment shows the exact pattern) |
| `trustedSigners` | leave empty for unsigned releases |

Validate before pushing (from `logos-modules-release-tool`): `./index.py validate-repo`.

## 3. Add the modules

```sh
./scripts/add-module.sh https://github.com/bristinWild/solicamp
git add -A && git commit -m "Add solicamp" && git push
```

**Check this carefully.** The script is built around one module per repo, with `metadata.json` and `flake.nix` at the repo root. SoliCamp has two modules in subfolders. Two ways to deal with it:

- **A. Split the repo (most reliable).** Create `bristinWild/solizone-module` and `bristinWild/solicamp-ui`, each with its folder contents at the root, and run `add-module.sh` for each. Then set the UI flake input back to `github:bristinWild/solizone-module`.
- **B. Keep one repo.** Run `add-module.sh` for `solicamp` once. The generated workflow passes `module_path: submodules/solicamp`. Make two copies of the generated `.github/workflows/release-solicamp.yml` and set `module_path: submodules/solicamp/solizone-module` and `module_path: submodules/solicamp/solicamp-ui`. This is not documented, so if the Actions run reports no `metadata.json`, fall back to A. Note that `release-all` discovers modules from `.gitmodules`, so it would see only one entry. Run the two module workflows by hand.

## 4. Release order matters

`solicamp_ui` depends on `solizone_module`. Release the core module first, then the UI.

## 5. Drop the Windows build

Our core module's flake only exposes macOS and Linux outputs. In `.github/workflows/_release-module.yml`, remove `windows-x86_64` from the `variants:` line. Otherwise a Windows leg is skipped or recorded as missing.

## 6. Run the release

In the fork, open **Actions**, choose **Release solizone_module** (or **Release all modules**), and run it. Or from the terminal: `./scripts/catalog.sh release-all`. Each run:

1. builds the `.lgx` for each variant,
2. verifies it (signing mode `none` means unsigned),
3. creates the release `solizone_module-v0.1.0`,
4. updates `index.json` in the `index` release.

Then run the UI workflow the same way. To re-release an unchanged version use **Force build** (`./scripts/catalog.sh release <module> --force`). For a new version, bump `version` in the module's `metadata.json`, update the submodule pointer, and run again.

## 7. Use the catalog

Point Basecamp's catalog setting at:

```text
https://raw.githubusercontent.com/bristinWild/logos-modules-release-base/main/logos-repo.json
```

Install **Solizone EVM** and **SoliCamp**, then open SoliCamp.

## Risks to watch in the first run

- **First CI build is slow.** The core module compiles Rust, REVM and the ZK libraries. Nix reads Logos' public cache at `cache.nix.logos.co`, which our flake already trusts.
- **`logos-rust-sdk-src`.** `Cargo.toml` has a path dependency on `../logos-rust-sdk-src`. On your Mac you stage it by hand for the lockfile. The comments say the Nix builder stages it during the build, but this has only been shown on your Mac. If CI fails with a missing `logos-rust-sdk-src` path, tell me and we will fix it in the flake.
- **Unpinned UI to core link.** The UI flake input follows the `main` branch of `bristinWild/solicamp`. Pin it to a tag or commit (`?dir=solizone-module&ref=<tag>`) once you release, so a UI build never mixes with a newer core.
- **Linux and Intel Mac are untested.** The flake lists `x86_64-linux`, `aarch64-linux`, `x86_64-darwin` and `aarch64-darwin`. Only Apple Silicon has been tried. Basecamp must also ship for the platform you release for.
- **Unsigned.** Releases are unsigned for now. To sign later, set the `LOGOS_SIGNING_KEY` Actions secret (Ed25519 key) and add the signer's DID to `trustedSigners`.
- **Unpublishing is permanent.** If you ever run `unpublish.yml`, run it with `dry_run: true` first.