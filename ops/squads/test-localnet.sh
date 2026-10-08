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
# npcs-treasury (#148), the real build, at its real id.
TREASURY_SO=${TREASURY_SO:-../../target/deploy/npcs_treasury.so}
[ -f "$TREASURY_SO" ] || { echo "build npcs-treasury first (anchor build -p npcs_treasury) or set TREASURY_SO" >&2; exit 1; }
TREASURY_PROGRAM=4BszRuAP4Gp7vdyu9MjE28gV8YTNtJZSRJRMQHdMut1y

# A tiny real program (SPL noop) to stand in for payment-rails /
# credit-gate, and a larger real one (memo) to upgrade to, so the upgrade
# also has to extend ProgramData. ExtendProgramChecked is deactivated to
# match devnet, where the permissionless ExtendProgram still works.
solana program dump -u devnet MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr "$TMP/memo.so" >/dev/null
solana program dump -u devnet noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV "$TMP/noop.so" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/program.json" >/dev/null
PROGRAM=$(solana-keygen pubkey "$TMP/program.json")
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/payer.json" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/upgrade.json" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$TMP/npcs-admin.json" >/dev/null

solana-test-validator --reset --quiet --ledger "$TMP/ledger" --rpc-port "$PORT" -u devnet \
  --clone-upgradeable-program "$SQUADS" --clone "$CONFIG_PDA" --clone "$TREASURY" \
  --clone-upgradeable-program TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP \
  --deactivate-feature 2oMRZEDWT2tqtYMofhmmfQ8SsjqUFzT6sYXppQDavxwz \
  --upgradeable-program "$PROGRAM" "$TMP/noop.so" "$TMP/upgrade.json" \
  --bpf-program "$TREASURY_PROGRAM" "$TREASURY_SO" >"$TMP/validator.log" 2>&1 &
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

# #171: a mint already wired to credit-gate (freeze authority = Token ACL
# MintConfig) moves everything else and leaves the freeze authority there.
MINT2=$(spl-token create-token -u "$URL" --program-2022 --decimals 6 --enable-freeze \
  --default-account-state frozen --enable-permanent-delegate --enable-metadata \
  --mint-authority "$TMP/npcs-admin.json" --fee-payer "$TMP/npcs-admin.json" --output json \
  | node -pe 'JSON.parse(require("fs").readFileSync(0)).commandOutput.address')
spl-token initialize-metadata -u "$URL" "$MINT2" NPCS NPCS https://example.invalid \
  --mint-authority "$TMP/npcs-admin.json" --update-authority "$(solana-keygen pubkey "$TMP/npcs-admin.json")" \
  --fee-payer "$TMP/npcs-admin.json" >/dev/null
MC=$(npx tsx testkit.ts acl-config "$MINT2" "$TMP/npcs-admin.json")
npx tsx migrate.ts move-mint "$MINT2" --authority "$TMP/npcs-admin.json"
npx tsx migrate.ts verify
SHOWN=$(spl-token display -u "$URL" "$MINT2")
grep -q "Freeze authority: $MC" <<<"$SHOWN" || fail "freeze authority left Token ACL"
grep -q "Mint authority: $(node -pe 'JSON.parse(require("fs").readFileSync(process.env.RECORD)).vault')" <<<"$SHOWN" \
  || fail "mint authority not with the vault"

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

# #172: the vault upgrades the program from a buffer it controls.
VAULT=$(node -pe 'JSON.parse(require("fs").readFileSync(process.env.RECORD)).vault')
pd_sha() { solana program dump -u "$URL" "$PROGRAM" "$TMP/pd.so" >/dev/null && sha256sum "$TMP/pd.so" | cut -d' ' -f1; }
BEFORE=$(pd_sha)
BUF=$(solana program write-buffer -u "$URL" "$TMP/memo.so" --keypair "$TMP/payer.json" --output json \
  | node -pe 'JSON.parse(require("fs").readFileSync(0)).buffer')
npx tsx migrate.ts propose-upgrade "$PROGRAM" --buffer "$BUF" >/dev/null 2>&1 \
  && fail "propose-upgrade accepted a buffer the vault does not control"
solana program set-buffer-authority -u "$URL" "$BUF" --buffer-authority "$TMP/payer.json" \
  --new-buffer-authority "$VAULT" --keypair "$TMP/payer.json" >/dev/null
npx tsx migrate.ts propose-upgrade "$PROGRAM" --buffer "$BUF"
AFTER=$(pd_sha)
[ "$AFTER" != "$BEFORE" ] || fail "program bytes unchanged after the vault upgrade"
# Compare the new build's length; anything after it must be zero padding.
N=$(stat -c %s "$TMP/memo.so")
cmp -n "$N" "$TMP/pd.so" "$TMP/memo.so" >/dev/null || fail "upgraded program is not the buffer's build"
[ -z "$(tail -c +$((N + 1)) "$TMP/pd.so" | tr -d '\0')" ] || fail "bytes after the new build are not zero padding"
node -e '
const r = JSON.parse(require("fs").readFileSync(process.env.RECORD, "utf8"));
const u = r.upgrades?.[0];
if (!u || u.approvals.length !== 2 || !u.executeSignature) { console.error("upgrade record incomplete", u); process.exit(1); }'

# One approval is not enough: a second proposal with only member-1 stays unexecutable.
npx tsx testkit.ts single-approval

# #148: the NPCS mint authority goes to npcs-treasury in one vault transaction.
# A third NPCS-shaped mint with 1.0 supply and the process-backing status, moved
# to the vault, and a vault-owned USDC sleeve holding the 1.0 backing.
MINT3=$(spl-token create-token -u "$URL" --program-2022 --decimals 6 --enable-freeze --enable-metadata \
  --mint-authority "$TMP/npcs-admin.json" --fee-payer "$TMP/npcs-admin.json" --output json \
  | node -pe 'JSON.parse(require("fs").readFileSync(0)).commandOutput.address')
ADMIN=$(solana-keygen pubkey "$TMP/npcs-admin.json")
spl-token initialize-metadata -u "$URL" "$MINT3" NPCS NPCS https://example.invalid \
  --mint-authority "$TMP/npcs-admin.json" --update-authority "$ADMIN" --fee-payer "$TMP/npcs-admin.json" >/dev/null
spl-token update-metadata -u "$URL" "$MINT3" status "TARGET-STATE: USDC 1:1 backing is a process rule, not enforced on-chain." \
  --authority "$TMP/npcs-admin.json" --fee-payer "$TMP/npcs-admin.json" >/dev/null
spl-token create-account -u "$URL" "$MINT3" --owner "$ADMIN" --fee-payer "$TMP/npcs-admin.json" >/dev/null
spl-token mint -u "$URL" "$MINT3" 1 --recipient-owner "$ADMIN" --mint-authority "$TMP/npcs-admin.json" --fee-payer "$TMP/npcs-admin.json" >/dev/null
npx tsx migrate.ts move-mint "$MINT3" --authority "$TMP/npcs-admin.json" >/dev/null
USDC=$(spl-token create-token -u "$URL" --decimals 6 --mint-authority "$TMP/payer.json" --fee-payer "$TMP/payer.json" --output json \
  | node -pe 'JSON.parse(require("fs").readFileSync(0)).commandOutput.address')
spl-token create-account -u "$URL" "$USDC" --owner "$VAULT" --fee-payer "$TMP/payer.json" >/dev/null
SLEEVE=$(spl-token address -u "$URL" --token "$USDC" --owner "$VAULT" --verbose | awk '/Associated token address/ {print $NF}')
spl-token mint -u "$URL" "$USDC" 0.5 "$SLEEVE" --mint-authority "$TMP/payer.json" --fee-payer "$TMP/payer.json" >/dev/null
npx tsx migrate.ts propose-treasury "$MINT3" --sleeve "$SLEEVE" >/dev/null 2>&1 \
  && fail "propose-treasury accepted a sleeve that cannot back the supply (I1)"
spl-token mint -u "$URL" "$USDC" 0.5 "$SLEEVE" --mint-authority "$TMP/payer.json" --fee-payer "$TMP/payer.json" >/dev/null
npx tsx migrate.ts propose-treasury "$MINT3" --sleeve "$SLEEVE"
SIGNER=$(node -pe 'JSON.parse(require("fs").readFileSync(process.env.RECORD)).treasury.signer')
TVAULT=$(node -pe 'JSON.parse(require("fs").readFileSync(process.env.RECORD)).treasury.vault')
SHOWN=$(spl-token display -u "$URL" "$MINT3")
grep -q "Mint authority: $SIGNER" <<<"$SHOWN" || fail "mint authority is not npcs-treasury's signer"
grep -q "enforced on-chain by npcs-treasury $TREASURY_PROGRAM" <<<"$SHOWN" || fail "metadata status not updated"
[ "$(spl-token balance -u "$URL" --address "$TVAULT")" = "1" ] || fail "backing USDC not in the treasury vault"
[ "$(spl-token balance -u "$URL" --address "$SLEEVE")" = "0" ] || fail "sleeve still holds the backing"
npx tsx migrate.ts verify
# Minting now happens only through deposit, 1:1, and I1 keeps holding.
PAYER_PK=$(solana-keygen pubkey "$TMP/payer.json")
spl-token create-account -u "$URL" "$USDC" --owner "$PAYER_PK" --fee-payer "$TMP/payer.json" >/dev/null
spl-token mint -u "$URL" "$USDC" 2 --recipient-owner "$PAYER_PK" --mint-authority "$TMP/payer.json" --fee-payer "$TMP/payer.json" >/dev/null
spl-token create-account -u "$URL" "$MINT3" --owner "$PAYER_PK" --fee-payer "$TMP/payer.json" >/dev/null
npx tsx testkit.ts treasury-deposit "$TREASURY_PROGRAM" "$MINT3" "$USDC" "$TMP/payer.json" 500000 >/dev/null
[ "$(spl-token supply -u "$URL" "$MINT3")" = "1.5" ] || fail "deposit did not mint 0.5"
[ "$(spl-token balance -u "$URL" --address "$TVAULT")" = "1.5" ] || fail "deposit did not land in the treasury vault"
npx tsx migrate.ts verify

echo "SQUADS MIGRATION TEST OK"
