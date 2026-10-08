#!/usr/bin/env bash
# Local demo cluster for the UI (#144): the real programs plus genuine
# program state, so every screen has something to read without devnet.
#
#   1. exports demo state with the LiteSVM harness
#      (payment-rails/tests-svm/tests/ui_fixture.rs): a printed 3-bidder
#      session and an open one with four sealed orders;
#   2. starts solana-test-validator with every program at its real id and
#      those accounts loaded.
#
# Needs `anchor build` at the repo root and in payment-rails/ first.
#
#   app/scripts/localnet.sh            # foreground; Ctrl-C stops it
#   COMMIT_SECS=120 app/scripts/localnet.sh
#
# Then, in app/: NEXT_PUBLIC_CLUSTER=localnet NEXT_PUBLIC_NPCS_MINT=<printed mint> npm run dev
# Demo wallets (throwaway, local only) are in $OUT/keys; import one into
# Phantom/Backpack to commit and reveal from the session page.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${OUT:-$ROOT/app/.localnet}"
LEDGER="${LEDGER:-$OUT/ledger}"
COMMIT_SECS="${COMMIT_SECS:-900}"
REVEAL_SECS="${REVEAL_SECS:-600}"

need() { [ -f "$1" ] || { echo "missing $1 — $2" >&2; exit 1; }; }
for p in auction_engine credit_gate mark_engine queue_claim liquidity_facility tranche_engine npcs_treasury; do
  need "$ROOT/target/deploy/$p.so" "run anchor build at the repo root"
done
need "$ROOT/payment-rails/target/deploy/payment_rails.so" "run anchor build in payment-rails/"
SAS_SO="$ROOT/programs/credit-gate/tests-svm/fixtures/sas.so"
need "$SAS_SO" "SAS fixture (dumped from devnet, pinned in credit-gate tests-svm)"

rm -rf "$OUT/accounts" "$OUT/keys" "$OUT/fixture.json" "$LEDGER"
mkdir -p "$OUT"
echo "exporting demo state (LiteSVM) -> $OUT"
UI_FIXTURE_OUT="$OUT" UI_FIXTURE_COMMIT_SECS="$COMMIT_SECS" UI_FIXTURE_REVEAL_SECS="$REVEAL_SECS" \
  cargo test --quiet --locked --manifest-path "$ROOT/payment-rails/tests-svm/Cargo.toml" \
  --test ui_fixture -- --ignored >/dev/null

id() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['address'])" "$1"; }
args=(--reset --quiet --ledger "$LEDGER")
for p in auction_engine credit_gate mark_engine queue_claim liquidity_facility tranche_engine npcs_treasury; do
  args+=(--bpf-program "$(id "$ROOT/target/idl/$p.json")" "$ROOT/target/deploy/$p.so")
done
args+=(--bpf-program "$(id "$ROOT/payment-rails/target/idl/payment_rails.json")" "$ROOT/payment-rails/target/deploy/payment_rails.so")
args+=(--bpf-program 22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG "$SAS_SO")
for f in "$OUT"/accounts/*.json; do
  args+=(--account "$(basename "$f" .json)" "$f")
done

MINT=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['assetMint'])" "$OUT/fixture.json")
cat <<EOF
validator: http://127.0.0.1:8899 (ledger $LEDGER)
in app/:   NEXT_PUBLIC_CLUSTER=localnet NEXT_PUBLIC_NPCS_MINT=$MINT npm run dev
fixture:   $OUT/fixture.json   wallets: $OUT/keys/
EOF
exec solana-test-validator "${args[@]}"
