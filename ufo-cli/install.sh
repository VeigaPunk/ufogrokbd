#!/bin/sh
# LTS pure-Rust installer for ufo-cli. No GH Actions. Installs via cargo.
set -e
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust toolchain required. Install from https://rustup.rs (stable LTS channel)."
  exit 1
fi
echo "[ufo] installing with LTS-pinned deps via cargo..."
cargo install --path . --locked --force
echo "[ufo] done. binary: $(command -v ufo || echo 'ufo (check PATH)')"
