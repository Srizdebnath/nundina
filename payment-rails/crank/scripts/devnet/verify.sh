#!/usr/bin/env bash
# Verify the live payment-rails program against its latest row in
# payment-rails/DEPLOYMENTS.md — no keys needed, so any fresh clone, auditor
# or reviewer can prove the deployed build is the recorded build (#29).
#
#   bash verify.sh                       # devnet
#   CLUSTER=mainnet-beta bash verify.sh
#   CLUSTER=devnet RPC_URL=https://… bash verify.sh
#
# Checks: dump SHA-256 == recorded Build SHA-256, on-chain upgrade authority
# == recorded authority, and (when the row records one) genesis hash.
set -euo pipefail
cd "$(dirname "$0")/../../.."   # → payment-rails/

CLUSTER=${CLUSTER:-devnet}
RPC=${RPC_URL:-$CLUSTER}
F=DEPLOYMENTS.md

die() { echo "✗ $*" >&2; exit 1; }
sol() { solana -u "$RPC" "$@"; }

# Latest registry row for $CLUSTER, fields looked up by column header so the
# table can gain columns without breaking this script.
field() {
  awk -F'|' -v cluster="$CLUSTER" -v want="$1" '
    /^\| Program \|/ { for (i = 2; i < NF; i++) { h = $i; gsub(/^ +| +$/, "", h); col[h] = i }; next }
    /^\|---/ { next }
    /^\|/ && ("Cluster" in col) {
      c = $col["Cluster"]; gsub(/^ +| +$/, "", c)
      if (c == cluster && (want in col)) { v = $col[want] }
    }
    END { if (v != "") { match(v, /`[^`]+`/); print substr(v, RSTART + 1, RLENGTH - 2) } }
  ' "$F"
}

PROGRAM_ID=$(field "Program ID")
[ -n "$PROGRAM_ID" ] || die "no $CLUSTER row in payment-rails/$F"
WANT_SHA=$(field "Build SHA-256")
WANT_AUTH=$(field "Upgrade authority")
WANT_GENESIS=$(field "Genesis hash")
echo "program   $PROGRAM_ID ($CLUSTER, from $F)"

if [ -n "$WANT_GENESIS" ]; then
  GENESIS=$(sol genesis-hash)
  [ "$GENESIS" = "$WANT_GENESIS" ] || die "genesis $GENESIS != recorded $WANT_GENESIS — wrong endpoint for $CLUSTER"
  echo "✓ genesis   $GENESIS"
fi

TMP=$(mktemp); trap 'rm -f "$TMP"' EXIT
sol program dump "$PROGRAM_ID" "$TMP" >/dev/null
GOT_SHA=$(sha256sum "$TMP" | cut -d' ' -f1)
[ "$GOT_SHA" = "$WANT_SHA" ] || die "on-chain SHA-256 $GOT_SHA != recorded $WANT_SHA"
echo "✓ build     $GOT_SHA"

GOT_AUTH=$(sol program show "$PROGRAM_ID" | awk '/^Authority:/ {print $2}')
[ "$GOT_AUTH" = "$WANT_AUTH" ] || die "upgrade authority $GOT_AUTH != recorded $WANT_AUTH"
echo "✓ authority $GOT_AUTH"
echo "✓ live program matches payment-rails/$F"
