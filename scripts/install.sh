#!/usr/bin/env bash
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${AGENTIC_HARNESS_PREFIX:-"$HOME/.agentic-harness"}"
BIN_DIR="$PREFIX/bin"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required to install Agentic Harness" >&2
  exit 1
fi

mkdir -p "$BIN_DIR"

if [[ "${AGENTIC_HARNESS_FROM_GIT:-0}" == "1" ]]; then
  REPO="${AGENTIC_HARNESS_REPO:-https://github.com/codejunkie99/agentic-harness.git}"
  REF="${AGENTIC_HARNESS_REF:-v0.1.1}"
  cargo install --git "$REPO" --tag "$REF" agentic-harness-cli --root "$PREFIX" --locked --force
else
  cargo install --path "$ROOT/crates/agentic-harness-cli" --root "$PREFIX" --locked --force
fi

echo "installed $("$BIN_DIR/agentic-harness" --version)"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "add this to PATH: export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
