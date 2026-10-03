#!/usr/bin/env bash
# End-to-end test of migrate.ts on a local validator running the REAL Squads
# v4 program (cloned from devnet with its ProgramConfig), against a
# throwaway upgradeable program and an NPCS-shaped Token-2022 mint (freeze,
# permanent delegate, metadata pointer + metadata). Timelock 0 so the
# propose → approve → execute proof completes in one run.
# Needs: solana-cli + spl-token on PATH, network access to devnet (clone).
set -euo pipefail
cd "$(dirname "$0")"

SQUADS=SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf
PORT=${PORT:-8899}
TMP=$(mktemp -d)
CONFIG_PDA=$(npx tsx testkit.ts config-pda)
TREASURY=$(npx tsx testkit.ts treasury)

# A tiny real program to stand in for payment-rails / credit-gate.
solana program dump -u devnet MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr "$TMP/memo.so" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/program.json" >/dev/null
PROGRAM=$(solana-keygen pubkey "$TMP/program.json")
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/payer.json" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/upgrade.json" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/npcs-admin.json" >/dev/null

solana-test-validator --reset --quiet --ledger "$TMP/ledger" --rpc-port "$PORT" -u devnet \
  --clone-upgradeable-program "$SQUADS" --clone "$CONFIG_PDA" --clone "$TREASURY" \
  --upgradeable-program "$PROGRAM" "$TMP/memo.so" "$TMP/upgrade.json" >"$TMP/validator.log" 2>&1 &
VPID=$!
trap 'kill $VPID 2>/dev/null || true; wait $VPID 2>/dev/null || true; rm -rf "$TMP"' EXIT
URL="http://127.0.0.1:$PORT"
for _ in $(seq 1 90); do solana -u "$URL" cluster-version >/dev/null 2>&1 && break; sleep 1; done

for k in payer npcs-admin upgrade; do solana airdrop -u "$URL" 10 "$TMP/$k.json" >/dev/null; done

# NPCS-shaped mint: every authority the real one has, held by one admin key.
MINT=$(spl-token create-token -u "$URL" --program-2022 --decimals 6 --enable-freeze \
  --enable-permanent-delegate --enable-metadata --mint-authority "$TMP/npcs-admin.json" \
  --fee-payer "$TMP/npcs-admin.json" --output json | node -pe 'JSON.parse(require("fs").readFileSync(0)).commandOutput.address')
spl-token initialize-metadata -u "$URL" "$MINT" NPCS NPCS https://example.invalid \
  --mint-authority "$TMP/npcs-admin.json" --update-authority "$(solana-keygen pubkey "$TMP/npcs-admin.json")" \
  --fee-payer "$TMP/npcs-admin.json" >/dev/null

export CLUSTER=localnet RPC_URL="$URL" PAYER="$TMP/payer.json" KEYS_DIR="$TMP/keys" \
  RECORD="$TMP/deployment-localnet.json" TIMELOCK_SECONDS=0
fail() { echo "FAIL: $*" >&2; exit 1; }

npx tsx migrate.ts create
npx tsx migrate.ts move-program "$PROGRAM" --authority "$TMP/upgrade.json"
npx tsx migrate.ts move-mint "$MINT" --authority "$TMP/npcs-admin.json"
npx tsx migrate.ts verify

# The old keys lost every power they had.
solana program set-upgrade-authority -u "$URL" "$PROGRAM" --upgrade-authority "$TMP/upgrade.json" \
  --new-upgrade-authority "$(solana-keygen pubkey "$TMP/upgrade.json")" --skip-new-upgrade-authority-signer-check \
  >/dev/null 2>&1 && fail "old upgrade key still controls the program"
spl-token mint -u "$URL" "$MINT" 1 --mint-authority "$TMP/npcs-admin.json" --fee-payer "$TMP/npcs-admin.json" \
  >/dev/null 2>&1 && fail "old admin key can still mint"
# A move is refused when the signer is not the holder (no silent partial custody).
npx tsx migrate.ts move-program "$PROGRAM" --authority "$TMP/upgrade.json" >/dev/null 2>&1 \
  && fail "move-program accepted a non-holder"

# The vault answers to 2-of-2: propose, both approve, execute.
npx tsx migrate.ts prove
node -e '
const r = JSON.parse(require("fs").readFileSync(process.env.RECORD, "utf8"));
const p = r.proofs[0];
if (!p || p.approvals.length !== 2 || !p.executeSignature) { console.error("proof incomplete", p); process.exit(1); }'

# One approval is not enough: a second proposal with only member-1 stays unexecutable.
npx tsx testkit.ts single-approval

echo "SQUADS MIGRATION TEST OK"
