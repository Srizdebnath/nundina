#!/usr/bin/env bash
# Deploy payment-rails at its fixed program id, verify the on-chain bytes,
# and record the deployment (devnet/mainnet only) in payment-rails/DEPLOYMENTS.md.
#
#   CLUSTER=devnet   AUTHORITY=~/.config/solana/id.json  bash deploy.sh
#   CLUSTER=http://127.0.0.1:8899                         bash deploy.sh   # rehearsal
#   CLUSTER=devnet RPC_URL=https://devnet.helius-rpc.com/?api-key=… bash deploy.sh
#
# RPC_URL overrides the endpoint while CLUSTER stays the recorded label: the
# public devnet RPC rate-limits the ~800 buffer writes of a full deploy.
# USE_RPC=0 sends those writes straight to leaders over QUIC instead, so a
# rate-limited RPC only serves the few reads and status polls.
#
# Idempotent: if the program already holds exactly this build, it does
# nothing. Upgrading a different build requires UPGRADE=1.
set -euo pipefail
cd "$(dirname "$0")/../../.."   # → payment-rails/

EXPECTED_ID=GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn
CLUSTER=${CLUSTER:-devnet}
RPC=${RPC_URL:-$CLUSTER}
AUTHORITY=${AUTHORITY:-$HOME/.config/solana/id.json}
SO=target/deploy/payment_rails.so
PROGRAM_KEY=keys/payment_rails-keypair.json
PRICE=${PRIORITY_FEE:-20000}   # µlamports / CU for deploy transactions
SEND_VIA=(--use-rpc); [ "${USE_RPC:-1}" = 0 ] && SEND_VIA=()

die() { echo "✗ $*" >&2; exit 1; }
sol() { solana -u "$RPC" "$@"; }

[ -f "$SO" ] || die "missing $SO — run 'anchor build' first"
[ -f "$PROGRAM_KEY" ] || die "missing $PROGRAM_KEY (the program keypair; back it up, it is gitignored)"
PROGRAM_ID=$(solana-keygen pubkey "$PROGRAM_KEY")
[ "$PROGRAM_ID" = "$EXPECTED_ID" ] || die "program keypair is $PROGRAM_ID, expected $EXPECTED_ID"
AUTH_PK=$(solana-keygen pubkey "$AUTHORITY")
LEN=$(stat -c %s "$SO")
BUILD_SHA=$(sha256sum "$SO" | cut -d' ' -f1)
echo "program   $PROGRAM_ID"
echo "cluster   $CLUSTER"
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
  devnet|mainnet-beta|d|m)
    F=DEPLOYMENTS.md
    if [ ! -f "$F" ]; then
      cat >"$F" <<'EOF'
# payment-rails deployments

Every row is verifiable: `solana program dump <id> x.so -u <cluster>` →
`sha256sum x.so` must equal the Build SHA-256; `solana program show <id>`
must show the Upgrade authority. Recorded by `crank/scripts/devnet/deploy.sh`.

| Program | Cluster | Program ID | Slot | Signature | Build SHA-256 | Upgrade authority |
|---|---|---|---|---|---|---|
EOF
    fi
    VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' programs/payment-rails/Cargo.toml | head -1)
    echo "| payment-rails v$VERSION ($MODE) | $CLUSTER | \`$PROGRAM_ID\` | $SLOT | \`${SIG:-n/a}\` | \`$BUILD_SHA\` | \`$AUTH_PK\` (single key, pilot; Squads 2-of-2 per SRS §8 before mainnet) |" >>"$F"
    echo "✓ recorded in payment-rails/$F"
    ;;
  *) echo "(local cluster — not recorded)";;
esac
