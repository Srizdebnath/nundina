#!/usr/bin/env bash
# Verify the live payment-rails program against its latest row in
# payment-rails/DEPLOYMENTS.md — no keys needed, so any fresh clone, auditor
# or reviewer can prove the deployed build is the recorded build (#29).
#
#   bash verify.sh                       # devnet
#   CLUSTER=mainnet-beta bash verify.sh
#   CLUSTER=devnet RPC_URL=https://… bash verify.sh
#   bash verify.sh --help                # this text; touches nothing
#
# Takes no arguments: anything other than --help aborts before any network
# access (#97). Never run it with `bash -x` while RPC_URL is set: the trace
# would print the URL, API key included, so the script refuses to.
#
# Fails closed on every check (#56):
#   - the row's program id must equal `declare_id!` in the program source,
#     so the registry can't vouch for a program id of its own choosing;
#   - the row must record a genesis hash, and the endpoint's must match it
#     (a row without one is not self-verifying and is rejected, never
#     skipped: skipping would let any endpoint, mainnet included, pass);
#   - the dump's first Bytes bytes hash to the recorded Build SHA-256, and
#     every byte after them is zero (ProgramData keeps its allocation after
#     an upgrade to a smaller build, so a healthy dump can be longer, #97);
#   - on-chain upgrade authority == recorded authority;
#   - on-chain "Last Deployed In Slot" == recorded Slot.
# Network reads are retried with backoff; "RPC kept failing" and "mismatch"
# are reported differently.
set -euo pipefail
SELF=$(realpath "$0")
. "$(dirname "$SELF")/common.sh"
refuse_xtrace_with_rpc_url
cd "$(dirname "$0")/../../.."   # → payment-rails/

die() { echo "✗ $*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$SELF"; }
for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument '$arg' (accepted: --help; configure with CLUSTER / RPC_URL / REGISTRY) — nothing was done" ;;
  esac
done

CLUSTER=${CLUSTER:-devnet}
RPC=${RPC_URL:-$CLUSTER}
RPC_SHOWN=$(rpc_shown "$RPC")   # the URL may carry an API key
F=${REGISTRY:-DEPLOYMENTS.md}
SRC=programs/payment-rails/src/lib.rs

sol() { solana -u "$RPC" --commitment confirmed "$@" 2> >(redact >&2); }

# retry <what> <cmd…>: run with 2/4/8/16 s backoff; exhaustion is fatal and
# says so (distinct from a value mismatch).
retry() {
  local what=$1 delay; shift
  for delay in 2 4 8 16 0; do
    "$@" && return 0
    [ "$delay" = 0 ] || { echo "  $what failed; retrying in ${delay}s" >&2; sleep "$delay"; }
  done
  die "$what failed 5 times at $RPC_SHOWN — RPC problem, NOT a verification result; retry or use RPC_URL"
}

# Latest registry row for $CLUSTER, fields looked up by column header so the
# table can gain columns without breaking this script.
field() {
  awk -F'|' -v cluster="$CLUSTER" -v want="$1" '
    /^\| Program \|/ { for (i = 2; i < NF; i++) { h = $i; gsub(/^ +| +$/, "", h); col[h] = i }; next }
    /^\|---/ { next }
    /^\|/ && ("Cluster" in col) {
      c = $col["Cluster"]; gsub(/^ +| +$/, "", c)
      if (c == cluster) { v = (want in col) ? $col[want] : "" }
    }
    END {
      gsub(/^ +| +$/, "", v)
      if (match(v, /`[^`]+`/)) print substr(v, RSTART + 1, RLENGTH - 2); else print v
    }
  ' "$F"
}

PROGRAM_ID=$(field "Program ID")
[ -n "$PROGRAM_ID" ] || die "no $CLUSTER row in payment-rails/$F"
WANT_SHA=$(field "Build SHA-256")
WANT_LEN=$(field "Bytes")
WANT_AUTH=$(field "Upgrade authority")
WANT_GENESIS=$(field "Genesis hash")
WANT_SLOT=$(field "Slot")
echo "program   $PROGRAM_ID ($CLUSTER, from $F)"

# 1. The registry may not choose the program id: it must be the source's.
SRC_ID=$(sed -n 's/^declare_id!("\([^"]*\)");/\1/p' "$SRC")
[ -n "$SRC_ID" ] || die "no declare_id! in $SRC"
[ "$PROGRAM_ID" = "$SRC_ID" ] || die "registry program id $PROGRAM_ID != declare_id! $SRC_ID in $SRC"
echo "✓ source    declare_id! matches"

# 2. Genesis is mandatory.
for v in WANT_GENESIS WANT_SHA WANT_LEN WANT_AUTH WANT_SLOT; do
  [ -n "${!v}" ] || die "the latest $CLUSTER row has no ${v#WANT_} value — it is not self-verifying; re-record it with deploy.sh"
done
case "$WANT_LEN" in *[!0-9]*) die "the latest $CLUSTER row's Bytes '$WANT_LEN' is not a byte count" ;; esac
GENESIS=
get_genesis() { GENESIS=$(sol genesis-hash) && [ -n "$GENESIS" ]; }
retry "genesis-hash" get_genesis
[ "$GENESIS" = "$WANT_GENESIS" ] || die "genesis $GENESIS != recorded $WANT_GENESIS — $RPC_SHOWN is not $CLUSTER"
echo "✓ genesis   $GENESIS"

# 3. Bytes: the recorded build, then nothing but zero padding.
TMP=$(mktemp); trap 'rm -f "$TMP"' EXIT
retry "program dump" sol program dump "$PROGRAM_ID" "$TMP" >/dev/null
GOT_LEN=$(wc -c <"$TMP" | tr -d ' ')
[ "$GOT_LEN" -ge "$WANT_LEN" ] || die "on-chain program is $GOT_LEN bytes, shorter than the recorded build ($WANT_LEN)"
PAD=$(tail -c +$((WANT_LEN + 1)) "$TMP" | tr -d '\0' | wc -c | tr -d ' ')
[ "$PAD" = 0 ] || die "$PAD non-zero byte(s) after the recorded $WANT_LEN-byte build — the live program is not the recorded build"
GOT_SHA=$(head -c "$WANT_LEN" "$TMP" | sha256sum | cut -d' ' -f1)
[ "$GOT_SHA" = "$WANT_SHA" ] || die "on-chain SHA-256 $GOT_SHA != recorded $WANT_SHA"
echo "✓ build     $GOT_SHA ($WANT_LEN bytes + $((GOT_LEN - WANT_LEN)) zero padding)"

# 4. Authority and slot.
SHOW=
get_show() { SHOW=$(sol program show "$PROGRAM_ID") && [ -n "$SHOW" ]; }
retry "program show" get_show
GOT_AUTH=$(awk '/^Authority:/ {print $2}' <<<"$SHOW")
[ "$GOT_AUTH" = "$WANT_AUTH" ] || die "upgrade authority $GOT_AUTH != recorded $WANT_AUTH"
echo "✓ authority $GOT_AUTH"
GOT_SLOT=$(awk '/Last Deployed In Slot:/ {print $NF}' <<<"$SHOW")
[ "$GOT_SLOT" = "$WANT_SLOT" ] || die "last deployed in slot $GOT_SLOT != recorded $WANT_SLOT — a newer (unrecorded) deploy, or a stale row"
echo "✓ slot      $GOT_SLOT"
echo "✓ live program matches payment-rails/$F"
