#!/usr/bin/env bash
# Copies the solizone-evm crate into rust-lib/vendor/solizone-evm without the
# Logos Zone SDK dependencies, so the Nix build only needs crates.io.
#
#   scripts/sync-solizone-evm.sh ~/logos/solizone/solizone-evm
#
# Run it again whenever solizone-evm changes, then regenerate rust-lib/Cargo.lock.
set -euo pipefail

SRC="${1:?usage: sync-solizone-evm.sh PATH_TO_solizone-evm}"
DST="$(cd "$(dirname "$0")/.." && pwd)/rust-lib/vendor/solizone-evm"

rm -rf "$DST"
mkdir -p "$DST"

cp -R "$SRC/src" "$DST/src"
cp -R "$SRC/contracts" "$DST/contracts"
rm -rf "$DST/src/bin"
cp "$SRC/LICENSE"* "$DST/" 2>/dev/null || true

# Cargo.toml: drop the Logos SDK dependencies, the features and the bin tables.
python3 - "$SRC/Cargo.toml" "$DST/Cargo.toml" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
text = re.sub(r'^lb-[^\n]*\n', '', text, flags=re.M)
text = re.sub(r'\[features\].*?(?=\[\[bin\]\]|\[dependencies\])', '', text, flags=re.S)
text = re.sub(r'\[\[bin\]\]\n(?:[^\n\[][^\n]*\n)*\n?', '', text)
open(sys.argv[2], 'w').write(text)
PY

echo "vendored $(basename "$SRC") into $DST"