#!/usr/bin/env bash
# Keyless verification of the live credit-gate program against its row in
# the root DEPLOYMENTS.md (#45). Read-only; no key needed.
#
#   bash programs/credit-gate/scripts/verify.sh            # devnet
#   CLUSTER=devnet RPC_URL=https://… bash programs/credit-gate/scripts/verify.sh
#
# Fails closed on: registry id != declare_id!; endpoint genesis != the
# row's cluster; dump SHA-256 != recorded Build SHA-256; upgrade authority
# or last-deployed slot != recorded. Then it reports (it does not fail)
# whether the live build equals a local `anchor build` of this checkout.
# If it doesn't, the source on this branch is not what's deployed, which is
# the #45 redeploy gap.
set -euo pipefail
cd "$(dirname "$0")/../../.."   # → repo root

CLUSTER=${CLUSTER:-devnet}
RPC=${RPC_URL:-$CLUSTER}
RPC_SHOWN=${RPC%%\?*}
F=${REGISTRY:-DEPLOYMENTS.md}
SRC=programs/credit-gate/src/lib.rs
LOCAL_SO=target/deploy/credit_gate.so

die() { echo "✗ $*" >&2; exit 1; }
sol() { solana -u "$RPC" --commitment confirmed "$@" 2> >(sed "s|${RPC//|/\\|}|$RPC_SHOWN|g" >&2); }
retry() {
  local what=$1 delay; shift
  for delay in 2 4 8 16 0; do
    "$@" && return 0
    [ "$delay" = 0 ] || { echo "  $what failed; retrying in ${delay}s" >&2; sleep "$delay"; }
  done
  die "$what failed 5 times at $RPC_SHOWN — RPC problem, NOT a verification result"
}

case "$CLUSTER" in
  devnet) WANT_GENESIS=EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG ;;
  mainnet-beta) WANT_GENESIS=5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d ;;
  testnet) WANT_GENESIS=4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY ;;
  *) die "CLUSTER must be devnet, testnet or mainnet-beta" ;;
esac

# The newest credit-gate row for $CLUSTER, fields by column header.
field() {
  awk -F'|' -v cluster="$CLUSTER" -v want="$1" '
    /^\| Program \|/ { delete col; for (i = 2; i < NF; i++) { h = $i; gsub(/^ +| +$/, "", h); col[h] = i }; next }
    /^\|---/ { next }
    /^\| *credit-gate/ && ("Cluster" in col) {
      c = $col["Cluster"]; gsub(/^ +| +$/, "", c)
      if (c == cluster) { v = (want in col) ? $col[want] : "" }
    }
    END { gsub(/^ +| +$/, "", v); if (match(v, /`[^`]+`/)) print substr(v, RSTART + 1, RLENGTH - 2); else print v }
  ' "$F"
}

PROGRAM_ID=$(field "Program ID")
[ -n "$PROGRAM_ID" ] || die "no credit-gate $CLUSTER row in $F"
WANT_SHA=$(field "Build SHA-256"); WANT_AUTH=$(field "Upgrade authority"); WANT_SLOT=$(field "Slot")
for v in WANT_SHA WANT_AUTH WANT_SLOT; do [ -n "${!v}" ] || die "credit-gate row has no ${v#WANT_} — not self-verifying"; done
echo "program   $PROGRAM_ID (credit-gate, $CLUSTER, from $F)"

SRC_ID=$(sed -n 's/^declare_id!("\([^"]*\)");/\1/p' "$SRC")
[ "$PROGRAM_ID" = "$SRC_ID" ] || die "registry program id $PROGRAM_ID != declare_id! $SRC_ID"
echo "✓ source    declare_id! matches"

GENESIS=; get_genesis() { GENESIS=$(sol genesis-hash) && [ -n "$GENESIS" ]; }
retry "genesis-hash" get_genesis
[ "$GENESIS" = "$WANT_GENESIS" ] || die "genesis $GENESIS is not $CLUSTER — wrong endpoint"
echo "✓ genesis   $GENESIS"

TMP=$(mktemp); trap 'rm -f "$TMP"' EXIT
retry "program dump" sol program dump "$PROGRAM_ID" "$TMP" >/dev/null
GOT_SHA=$(sha256sum "$TMP" | cut -d' ' -f1)
[ "$GOT_SHA" = "$WANT_SHA" ] || die "on-chain SHA-256 $GOT_SHA != recorded $WANT_SHA"
echo "✓ build     $GOT_SHA"

SHOW=; get_show() { SHOW=$(sol program show "$PROGRAM_ID") && [ -n "$SHOW" ]; }
retry "program show" get_show
GOT_AUTH=$(awk '/^Authority:/ {print $2}' <<<"$SHOW")
[ "$GOT_AUTH" = "$WANT_AUTH" ] || die "upgrade authority $GOT_AUTH != recorded $WANT_AUTH"
echo "✓ authority $GOT_AUTH"
GOT_SLOT=$(awk '/Last Deployed In Slot:/ {print $NF}' <<<"$SHOW")
[ "$GOT_SLOT" = "$WANT_SLOT" ] || die "last deployed in slot $GOT_SLOT != recorded $WANT_SLOT"
echo "✓ slot      $GOT_SLOT"
echo "✓ live credit-gate matches $F"

# Informational: is this checkout's source what's deployed?
if [ -f "$LOCAL_SO" ]; then
  LOCAL_SHA=$(sha256sum "$LOCAL_SO" | cut -d' ' -f1)
  if [ "$LOCAL_SHA" = "$GOT_SHA" ]; then echo "ℹ local anchor build == live build"
  else echo "⚠ local anchor build $LOCAL_SHA != live build: this checkout's credit-gate is NOT what is deployed (redeploy pending, #45)"; fi
else
  echo "ℹ no $LOCAL_SO — run 'anchor build' to compare this checkout with the live build"
fi
