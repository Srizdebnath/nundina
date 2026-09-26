#!/usr/bin/env bash
# Devnet dress rehearsal on a local validator: the SAME deploy.sh, setup.ts
# and demo.ts that run against devnet, with a local 6-dp stand-in for USDC
# (Circle's devnet mint doesn't exist locally). The program is NOT preloaded
# — deploy.sh deploys it with the real program keypair. Deploy and setup are
# then re-run to prove they are idempotent.
set -euo pipefail
cd "$(dirname "$0")/../.."   # → crank/
PORT=${PORT:-8899}
URL="http://127.0.0.1:$PORT"
TMP=$(mktemp -d)
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/authority.json" >/dev/null
solana-test-validator --reset --quiet --ledger "$TMP/ledger" --rpc-port "$PORT" >"$TMP/validator.log" 2>&1 &
VPID=$!
trap 'kill $VPID 2>/dev/null || true; wait $VPID 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do solana -u "$URL" cluster-version >/dev/null 2>&1 && break; sleep 1; done
AUTH_PK=$(solana-keygen pubkey "$TMP/authority.json")
solana -u "$URL" airdrop 50 "$AUTH_PK" >/dev/null

echo "── local USDC stand-in"
USDC=$(spl-token -u "$URL" create-token --decimals 6 --fee-payer "$TMP/authority.json" --mint-authority "$TMP/authority.json" --output json | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>console.log(JSON.parse(s).commandOutput.address))")
for _ in $(seq 1 30); do solana -u "$URL" account "$USDC" --commitment confirmed >/dev/null 2>&1 && break; sleep 1; done
spl-token -u "$URL" create-account "$USDC" --owner "$AUTH_PK" --fee-payer "$TMP/authority.json" >/dev/null
spl-token -u "$URL" mint "$USDC" 20 --recipient-owner "$AUTH_PK" --mint-authority "$TMP/authority.json" --fee-payer "$TMP/authority.json" >/dev/null
echo "USDC stand-in $USDC (20 minted to authority)"

echo; echo "── deploy.sh (fresh)"
CLUSTER="$URL" AUTHORITY="$TMP/authority.json" bash scripts/devnet/deploy.sh
echo; echo "── deploy.sh (again: must be a no-op)"
CLUSTER="$URL" AUTHORITY="$TMP/authority.json" bash scripts/devnet/deploy.sh | tail -1

echo; echo "── setup.ts (fresh)"
RPC_URL="$URL" AUTHORITY="$TMP/authority.json" USDC_MINT="$USDC" npx tsx scripts/devnet/setup.ts | grep -E "✓|cluster"
echo; echo "── setup.ts (again: must only verify)"
RPC_URL="$URL" AUTHORITY="$TMP/authority.json" USDC_MINT="$USDC" npx tsx scripts/devnet/setup.ts | grep -E "✓"

echo; echo "── demo.ts"
RPC_URL="$URL" AUTHORITY="$TMP/authority.json" COMMIT_SECS=${COMMIT_SECS:-20} REVEAL_SECS=${REVEAL_SECS:-12} \
  npx tsx scripts/devnet/demo.ts
echo "REHEARSAL OK"
