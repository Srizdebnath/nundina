#!/usr/bin/env bash
# Deploy payment-rails at its fixed program id, verify the on-chain bytes,
# and record the deployment (devnet/mainnet only) in payment-rails/DEPLOYMENTS.md.
#
#   CLUSTER=devnet   AUTHORITY=~/.config/solana/id.json  bash deploy.sh
#   CLUSTER=http://127.0.0.1:8899                         bash deploy.sh   # rehearsal
#   CLUSTER=devnet RPC_URL=https://devnet.helius-rpc.com/?api-key=… bash deploy.sh
#   RPC_URL=… bash deploy.sh --dry-run    # print (RPC, genesis, cluster) and exit
#
# RPC_URL overrides the endpoint (the public devnet RPC rate-limits the ~800
# buffer writes of a full deploy). The recorded cluster label is NEVER taken
# from the caller: it is derived from the endpoint's genesis hash, and a
# CLUSTER moniker that disagrees with it aborts the run (#28). Mainnet also
# requires I_MEAN_MAINNET=1.
# USE_RPC=0 sends those writes straight to leaders over QUIC instead, so a
# rate-limited RPC only serves the few reads and status polls.
#
# Idempotent: if the program already holds exactly this build, it does
# nothing. Upgrading a different build requires UPGRADE=1.
set -euo pipefail
cd "$(dirname "$0")/../../.."   # → payment-rails/

EXPECTED_ID=GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn
REQUESTED=${CLUSTER:-}   # endpoint shorthand/URL, or a moniker to assert
RPC=${RPC_URL:-${REQUESTED:-devnet}}
RPC_SHOWN=${RPC%%\?*}   # for output: a query string may carry an API key
DRY_RUN=${DRY_RUN:-0}; [ "${1:-}" = --dry-run ] && DRY_RUN=1
AUTHORITY=${AUTHORITY:-$HOME/.config/solana/id.json}
SO=target/deploy/payment_rails.so
PROGRAM_KEY=keys/payment_rails-keypair.json
PRICE=${PRIORITY_FEE:-20000}   # µlamports / CU for deploy transactions
SEND_VIA=(--use-rpc); [ "${USE_RPC:-1}" = 0 ] && SEND_VIA=()

die() { echo "✗ $*" >&2; exit 1; }
sol() { solana -u "$RPC" "$@"; }

# ── Resolve the cluster from the endpoint itself (#28) ──────────────────
# Genesis hashes are the clusters' identities; a local validator gets a
# fresh random one per ledger, so "local" is recognised by a loopback
# endpoint instead. Anything else is refused rather than guessed.
GENESIS=
for _ in 1 2 3; do GENESIS=$(sol genesis-hash 2>/dev/null) && break; GENESIS=; sleep 2; done
[ -n "$GENESIS" ] || die "cannot reach $RPC_SHOWN (solana genesis-hash failed)"
case "$GENESIS" in
  5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d) CLUSTER=mainnet-beta ;;
  EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG) CLUSTER=devnet ;;
  4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY) CLUSTER=testnet ;;
  *)
    case "$RPC" in
      l|localhost|http://127.0.0.1:*|http://localhost:*|http://\[::1\]:*) CLUSTER=local ;;
      *) die "unrecognised cluster at $RPC_SHOWN (genesis $GENESIS) — refusing to deploy or record it" ;;
    esac ;;
esac
case "$REQUESTED" in
  devnet|d) WANT=devnet ;; mainnet-beta|m) WANT=mainnet-beta ;;
  testnet|t) WANT=testnet ;; localhost|l) WANT=local ;; *) WANT= ;;
esac
[ -z "$WANT" ] || [ "$WANT" = "$CLUSTER" ] \
  || die "CLUSTER=$REQUESTED but $RPC_SHOWN is $CLUSTER (genesis $GENESIS) — unset RPC_URL or fix CLUSTER"
echo "rpc       $RPC_SHOWN"
echo "genesis   $GENESIS"
echo "cluster   $CLUSTER"
if [ "$DRY_RUN" = 1 ]; then echo "(dry run — nothing sent, nothing recorded)"; exit 0; fi
if [ "$CLUSTER" = mainnet-beta ] && [ "${I_MEAN_MAINNET:-0}" != 1 ]; then
  die "refusing mainnet-beta without I_MEAN_MAINNET=1 (upgrade authority must be Squads 2-of-2 first, SRS §8)"
fi

[ -f "$SO" ] || die "missing $SO — run 'anchor build' first"
[ -f "$PROGRAM_KEY" ] || die "missing $PROGRAM_KEY (the program keypair; back it up, it is gitignored)"
PROGRAM_ID=$(solana-keygen pubkey "$PROGRAM_KEY")
[ "$PROGRAM_ID" = "$EXPECTED_ID" ] || die "program keypair is $PROGRAM_ID, expected $EXPECTED_ID"
AUTH_PK=$(solana-keygen pubkey "$AUTHORITY")
LEN=$(stat -c %s "$SO")
BUILD_SHA=$(sha256sum "$SO" | cut -d' ' -f1)
echo "program   $PROGRAM_ID"
echo "authority $AUTH_PK"
echo "build     $BUILD_SHA ($LEN bytes)"

verify_dump() {
  local tmp; tmp=$(mktemp)
  sol program dump "$PROGRAM_ID" "$tmp" >/dev/null
  local got; got=$(sha256sum "$tmp" | cut -d' ' -f1); rm -f "$tmp"
  echo "$got"
}

if sol program show "$PROGRAM_ID" >/dev/null 2>&1; then
  ONCHAIN=$(verify_dump)
  if [ "$ONCHAIN" = "$BUILD_SHA" ]; then
    echo "✓ already deployed with this exact build — nothing to do"
    exit 0
  fi
  [ "${UPGRADE:-0}" = 1 ] || die "program exists with a different build ($ONCHAIN); re-run with UPGRADE=1 to upgrade"
  CURRENT_AUTH=$(sol program show "$PROGRAM_ID" | awk '/^Authority:/ {print $2}')
  [ "$CURRENT_AUTH" = "$AUTH_PK" ] || die "upgrade authority is $CURRENT_AUTH, not $AUTH_PK"
  MODE=upgrade
else
  MODE=deploy
fi

# Cost check: ProgramData rent (+ an equal-size buffer during the write,
# refunded at the end) + fees margin.
RENT=$(sol rent $((LEN + 45)) --lamports | awk '{print $NF == "lamports" ? $(NF-1) : $NF}' | tr -dc 0-9)
NEED=$((RENT * 2 + 200000000))
BAL=$(sol balance "$AUTH_PK" --lamports | tr -dc 0-9)
echo "rent      $((RENT / 1000000)) mSOL · need ≈ $((NEED / 1000000)) mSOL · have $((BAL / 1000000)) mSOL"
if [ "$BAL" -lt "$NEED" ]; then
  die "authority needs ≈ $(( (NEED - BAL) / 1000000 + 1 )) more mSOL — fund $AUTH_PK (devnet: https://faucet.solana.com) and re-run"
fi

echo "→ $MODE"
OUT=$(sol program deploy "$SO" \
  --program-id "$PROGRAM_KEY" \
  --upgrade-authority "$AUTHORITY" \
  --keypair "$AUTHORITY" \
  --max-len "$LEN" \
  --with-compute-unit-price "$PRICE" \
  --max-sign-attempts 50 \
  "${SEND_VIA[@]}" 2>&1) || { echo "$OUT" >&2; die "deploy failed (a partial buffer may hold SOL: 'solana program show --buffers' / 'solana program close --buffers')"; }
echo "$OUT"
SIG=$(echo "$OUT" | awk '/^Signature:/ {print $2}')

ONCHAIN=$(verify_dump)
[ "$ONCHAIN" = "$BUILD_SHA" ] || die "on-chain dump $ONCHAIN != build $BUILD_SHA"
SHOW=$(sol program show "$PROGRAM_ID")
SLOT=$(echo "$SHOW" | awk '/Last Deployed In Slot:/ {print $NF}')
ONCHAIN_AUTH=$(echo "$SHOW" | awk '/^Authority:/ {print $2}')
[ "$ONCHAIN_AUTH" = "$AUTH_PK" ] || die "upgrade authority on-chain is $ONCHAIN_AUTH"
echo "✓ verified: dump SHA-256 == build, authority $ONCHAIN_AUTH, slot $SLOT"

case "$CLUSTER" in
  devnet|mainnet-beta)
    F=DEPLOYMENTS.md
    if [ ! -f "$F" ]; then
      cat >"$F" <<'EOF'
# payment-rails deployments

Every row is verifiable: `solana genesis-hash -u <cluster>` must equal the
Genesis hash; `solana program dump <id> x.so -u <cluster>` → `sha256sum x.so`
must equal the Build SHA-256; `solana program show <id>` must show the
Upgrade authority. Recorded by `crank/scripts/devnet/deploy.sh`, which
derives the Cluster from the genesis hash (never from the caller).

| Program | Cluster | Genesis hash | Program ID | Slot | Signature | Build SHA-256 | Upgrade authority |
|---|---|---|---|---|---|---|---|
EOF
    fi
    VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' programs/payment-rails/Cargo.toml | head -1)
    echo "| payment-rails v$VERSION ($MODE) | $CLUSTER | \`$GENESIS\` | \`$PROGRAM_ID\` | $SLOT | \`${SIG:-n/a}\` | \`$BUILD_SHA\` | \`$AUTH_PK\` (single key, pilot; Squads 2-of-2 per SRS §8 before mainnet) |" >>"$F"
    echo "✓ recorded in payment-rails/$F"
    ;;
  *) echo "(local cluster — not recorded)";;
esac
