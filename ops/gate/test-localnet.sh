#!/usr/bin/env bash
# End-to-end test of the key holders' path on a local validator:
# credit-gate (this checkout's build) at its real id, the REAL Token ACL and
# SAS programs cloned from devnet, an NPCS-shaped mint (DefaultAccountState
# = Frozen). sas.ts setup → wire.ts root/market/acl/verify → sas.ts attest →
# wire.ts validate → a stranger's permissionless thaw. Plus: an unattested
# wallet can't be thawed, and the old freeze key no longer controls freezes.
# Needs: `anchor build` at the repo root, solana-cli + spl-token, devnet access.
set -euo pipefail
cd "$(dirname "$0")"
GATE=5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX
SO=../../target/deploy/credit_gate.so
[ -f "$SO" ] || { echo "missing $SO — run 'anchor build' at the repo root" >&2; exit 1; }
PORT=${PORT:-8899}
TMP=$(mktemp -d)
for k in payer upgrade gov issuer stranger; do
  solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/$k.json" >/dev/null
done
solana-test-validator --reset --quiet --ledger "$TMP/ledger" --rpc-port "$PORT" -u devnet \
  --clone-upgradeable-program TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP \
  --clone-upgradeable-program 22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG \
  --upgradeable-program "$GATE" "$SO" "$TMP/upgrade.json" >"$TMP/validator.log" 2>&1 &
VPID=$!
trap 'kill $VPID 2>/dev/null || true; wait $VPID 2>/dev/null || true; rm -rf "$TMP"; rm -f deployment-localnet.json' EXIT
URL="http://127.0.0.1:$PORT"
for _ in $(seq 1 90); do solana -u "$URL" cluster-version >/dev/null 2>&1 && break; sleep 1; done
for k in payer upgrade gov issuer stranger; do solana airdrop -u "$URL" 20 "$TMP/$k.json" >/dev/null; done
export CLUSTER=localnet RPC_URL="$URL" PAYER="$TMP/payer.json" KEYS_DIR="$TMP/sas-keys"
fail() { echo "FAIL: $*" >&2; exit 1; }
pk() { solana-keygen pubkey "$TMP/$1.json"; }

npx tsx sas.ts setup >/dev/null && npx tsx sas.ts verify

# NPCS-shaped mint: frozen by default, freeze authority = the issuer key.
MINT=$(spl-token create-token -u "$URL" --program-2022 --decimals 6 --enable-freeze \
  --default-account-state frozen --mint-authority "$TMP/issuer.json" --fee-payer "$TMP/issuer.json" \
  --output json | node -pe 'JSON.parse(require("fs").readFileSync(0)).commandOutput.address')

npx tsx wire.ts verify --mint "$MINT" >/dev/null 2>&1 && fail "verify passed before wiring"
npx tsx wire.ts root --upgrade-authority "$TMP/upgrade.json" --root "$(pk gov)" >/dev/null
npx tsx wire.ts market --authority "$TMP/gov.json" --mint "$MINT" --jurisdictions US --min-kyc 1 >/dev/null
npx tsx wire.ts acl --freeze-authority "$TMP/issuer.json" --mint "$MINT" >/dev/null
npx tsx wire.ts verify --mint "$MINT"

# An attested investor, and one with no attestations.
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/alice.json" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/mallory.json" >/dev/null
ALICE=$(pk alice); MALLORY=$(pk mallory)
npx tsx sas.ts attest "$ALICE" --investor 0192f0a0-1c2b-7d3e-8f40-5a6b7c8d9e0f --jurisdiction US --kyc 2 >/dev/null
npx tsx wire.ts validate "$ALICE" --mint "$MINT" >/dev/null

# Plain (non-ATA) accounts with known addresses; state read from the
# account bytes (Token-2022 base layout: state at offset 108, 2 = frozen).
acct() {
  solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/ta-$1.json" >/dev/null
  spl-token create-account -u "$URL" "$MINT" "$TMP/ta-$1.json" --owner "$1" --fee-payer "$TMP/payer.json" >/dev/null
  solana-keygen pubkey "$TMP/ta-$1.json"
}
state() {
  solana account -u "$URL" "$1" --output json \
    | node -pe 'Buffer.from(JSON.parse(require("fs").readFileSync(0)).account.data[0], "base64")[108] === 2'
}
TA=$(acct "$ALICE"); TM=$(acct "$MALLORY")
[ "$(state "$TA")" = true ] || fail "new account not born frozen"

# The stranger pays for both thaws: no gate or issuer key involved.
PAYER="$TMP/stranger.json" npx tsx wire.ts thaw "$TA" --mint "$MINT" >/dev/null
[ "$(state "$TA")" = false ] || fail "attested investor not thawed"
PAYER="$TMP/stranger.json" npx tsx wire.ts thaw "$TM" --mint "$MINT" >/dev/null 2>&1 && fail "unattested wallet thawed"
[ "$(state "$TM")" = true ] || fail "unattested account changed state"

# The issuer's old freeze key is no longer the freeze authority.
spl-token freeze -u "$URL" "$TA" --freeze-authority "$TMP/issuer.json" --fee-payer "$TMP/issuer.json" \
  >/dev/null 2>&1 && fail "old freeze key still freezes"

echo "GATE WIRING TEST OK"
