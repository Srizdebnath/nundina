#!/usr/bin/env bash
# Install solana-cli (agave) and the anchor CLI at their pinned versions,
# refusing any artifact whose SHA-256 differs from the one recorded here
# (#98). The old path piped `release.anza.xyz/.../install` into `sh` and
# curl'd the anchor binary with no checksum: a moved or compromised release
# asset would silently change the binary that builds the audit-scope
# programs. Bumping a version means bumping its hash here, deliberately.
#
# Usage (CI): SOLANA_VERSION=2.3.0 ANCHOR_VERSION=1.1.2 bash install-solana-anchor.sh
# Adds both to $GITHUB_PATH when set.
set -euo pipefail

: "${SOLANA_VERSION:?}" "${ANCHOR_VERSION:?}"

case "$SOLANA_VERSION" in
  2.3.0) SOLANA_SHA=56241fbe862495ff01b2b875195e44f94c22e9f2a504591a3ade1b9d82862730 ;;
  *) echo "::error::no pinned SHA-256 for agave $SOLANA_VERSION — add it to $0"; exit 1 ;;
esac
case "$ANCHOR_VERSION" in
  1.1.2) ANCHOR_SHA=fdea9979629e9416e5f5e5622ff6c11b8c691d1e559581ece368e903c0c980c1 ;;
  *) echo "::error::no pinned SHA-256 for anchor $ANCHOR_VERSION — add it to $0"; exit 1 ;;
esac

ROOT=${INSTALL_ROOT:-$HOME/.local}
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
mkdir -p "$ROOT/bin" "$ROOT/share"

fetch() {  # fetch <url> <out> <sha256>
  curl -sSfL --retry 3 -o "$2" "$1"
  echo "$3  $2" | sha256sum -c --quiet - \
    || { echo "::error::SHA-256 mismatch for $1"; sha256sum "$2"; exit 1; }
}

fetch "https://github.com/anza-xyz/agave/releases/download/v${SOLANA_VERSION}/solana-release-x86_64-unknown-linux-gnu.tar.bz2" \
  "$TMP/solana.tar.bz2" "$SOLANA_SHA"
rm -rf "$ROOT/share/solana-release"
tar -xjf "$TMP/solana.tar.bz2" -C "$ROOT/share"

fetch "https://github.com/solana-foundation/anchor/releases/download/v${ANCHOR_VERSION}/anchor-${ANCHOR_VERSION}-x86_64-unknown-linux-gnu" \
  "$ROOT/bin/anchor" "$ANCHOR_SHA"
chmod +x "$ROOT/bin/anchor"

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$ROOT/share/solana-release/bin" >> "$GITHUB_PATH"
  echo "$ROOT/bin" >> "$GITHUB_PATH"
fi
"$ROOT/share/solana-release/bin/solana" --version
"$ROOT/bin/anchor" --version
