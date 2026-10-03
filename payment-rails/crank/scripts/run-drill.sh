#!/usr/bin/env bash
# Kill-the-crank drill: fresh solana-test-validator with the built program
# (upgradeable, throwaway upgrade authority), run scripts/drill.ts, tear down.
# Needs: `anchor build` in payment-rails/, solana-cli on PATH, npm install.
set -euo pipefail
cd "$(dirname "$0")/.."

PROGRAM_ID=GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn
SO=../target/deploy/payment_rails.so
PORT=${PORT:-8899}
[ -f "$SO" ] || { echo "missing $SO — run 'anchor build' in payment-rails/ first" >&2; exit 1; }

TMP=$(mktemp -d)
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/authority.json" >/dev/null
solana-test-validator --reset --quiet --ledger "$TMP/ledger" --rpc-port "$PORT" \
  --upgradeable-program "$PROGRAM_ID" "$SO" "$TMP/authority.json" >"$TMP/validator.log" 2>&1 &
VPID=$!
trap 'kill $VPID 2>/dev/null || true; wait $VPID 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  solana -u "http://127.0.0.1:$PORT" cluster-version >/dev/null 2>&1 && break
  kill -0 $VPID 2>/dev/null || { echo "validator died:" >&2; tail -20 "$TMP/validator.log" >&2; exit 1; }
  sleep 1
done

EVIDENCE_DIR=${EVIDENCE_DIR:-$TMP/evidence}
export RPC_URL="http://127.0.0.1:$PORT"
AUTHORITY="$TMP/authority.json" EVIDENCE_DIR="$EVIDENCE_DIR" DRILL_OUT="$TMP/drill.json" \
  npx tsx scripts/drill.ts

# Exercise the CLI itself against the drill's final state.
SESSION=$(node -p "require('$TMP/drill.json').session")
FACILITY=$(node -p "require('$TMP/drill.json').facility")
echo; echo "── CLI: reconcile / refunds / export ──"
KEYPAIR="$TMP/authority.json" npx tsx src/cli.ts reconcile --session "$SESSION"
KEYPAIR="$TMP/authority.json" npx tsx src/cli.ts reconcile --facility "$FACILITY"
KEYPAIR="$TMP/authority.json" npx tsx src/cli.ts refunds --session "$SESSION"
KEYPAIR="$TMP/authority.json" npx tsx src/cli.ts export --out "$TMP/cli-evidence" >/dev/null
echo "CLI export: $(node -p "require('$TMP/cli-evidence/manifest.json').events") events"
# #127: the CLI bundle is signed by the exporting key and verifies offline;
# naming another signer must fail.
SIGNER=$(node -p "require('$TMP/cli-evidence/manifest.json').attestation.signer")
npx tsx src/cli.ts verify-export --dir "$TMP/cli-evidence" --signer "$SIGNER"
if npx tsx src/cli.ts verify-export --dir "$TMP/cli-evidence" --signer 11111111111111111111111111111111 >/dev/null; then
  echo "verify-export accepted the wrong signer"; exit 1
fi
echo "DRILL OK"
