#!/usr/bin/env bash
# Dress rehearsal of the live-NPCS runs (#145, #172) on a local clone of
# devnet: the real credit-gate, Token ACL, SAS, auction-engine and
# mark-engine programs and every devnet account the run touches (NPCS mint +
# MintConfig, gate market, holder H's link / attestations / NPCS account,
# rails config, Circle USDC mint, venue, feed), with payment-rails loaded
# from the pending Squads upgrade's buffer (#3) — the exact build devnet
# will run. Two accounts are patched, nothing else: H's lot ledger
# (seasoning already passed) and the operator's USDC balance
# (scripts/devnet/npcs-clone.ts says why). Then the script runs unchanged.
#
#   bash scripts/devnet/rehearse-npcs.sh                  # npcs-dvp.ts: rails-direct DvP
#   bash scripts/devnet/rehearse-npcs.sh auction-session  # the sealed-bid auction-engine session
#
# AUCTION_SO=<path to auction_engine.so> loads that build at auction-engine's
# id (upgrade authority 9WH1, as on devnet) instead of cloning the deployed
# one: the rehearsal of a pending auction-engine upgrade.
set -euo pipefail
cd "$(dirname "$0")/../.."   # → crank/
RUN=${1:-npcs-dvp}
case "$RUN" in npcs-dvp|auction-session) ;; *) echo "usage: $0 [npcs-dvp|auction-session]" >&2; exit 2 ;; esac
PORT=${PORT:-8899}
URL="http://127.0.0.1:$PORT"
DEVNET=${DEVNET_RPC:-https://api.devnet.solana.com}
TMP=$(mktemp -d -p "${TMPDIR:-$HOME}")
export NODE_OPTIONS=--dns-result-order=ipv4first

echo "── clone plan (from devnet)"
RPC_URL="$DEVNET" npx tsx scripts/devnet/npcs-clone.ts "$TMP"
echo "rails.so sha256 $(sha256sum "$TMP/rails.so" | cut -d' ' -f1)"

args=(--reset --quiet --ledger "$TMP/ledger" --rpc-port "$PORT" --url "$DEVNET"
  --faucet-port "${FAUCET_PORT:-9910}" --gossip-port "${GOSSIP_PORT:-8010}" --dynamic-port-range "${DYNAMIC_PORTS:-8020-8070}")
prog() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['programs'][sys.argv[2]])" "$TMP/plan.json" "$1"; }
for p in gate tokenAcl sas mark; do
  args+=(--clone-upgradeable-program "$(prog "$p")")
done
if [ -n "${AUCTION_SO:-}" ]; then
  echo "auction_engine.so (pending upgrade) sha256 $(sha256sum "$AUCTION_SO" | cut -d' ' -f1)"
  args+=(--upgradeable-program "$(prog auction)" "$AUCTION_SO" 9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ)
else
  args+=(--clone-upgradeable-program "$(prog auction)")
fi
RAILS=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['programs']['rails'])" "$TMP/plan.json")
args+=(--upgradeable-program "$RAILS" "$TMP/rails.so" 8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV)
for a in $(python3 -c "import json,sys; print(' '.join(json.load(open(sys.argv[1]))['clone']))" "$TMP/plan.json"); do args+=(--maybe-clone "$a"); done
args+=(--account - "$TMP/lots.json" --account - "$TMP/usdc.json")

solana-test-validator "${args[@]}" >"$TMP/validator.log" 2>&1 &
VPID=$!
STATE=.devnet/localnet
BACKUP=""
cleanup() {
  kill $VPID 2>/dev/null || true; wait $VPID 2>/dev/null || true
  if [ -n "$BACKUP" ]; then mv "$BACKUP" "$STATE/deployment.json"; else rm -f "$STATE/deployment.json"; fi
}
trap cleanup EXIT
for _ in $(seq 1 90); do solana -u "$URL" cluster-version >/dev/null 2>&1 && break; sleep 1; done

# npcs-dvp.ts reads the deployment record by cluster label; the clone is
# "localnet" but its rails config IS devnet's.
mkdir -p "$STATE"
if [ -f "$STATE/deployment.json" ]; then BACKUP="$TMP/localnet-deployment.json"; cp "$STATE/deployment.json" "$BACKUP"; fi
python3 - "$URL" <<'PY'
import json, sys
d = json.load(open(".devnet/devnet/deployment.json"))
d["rpcUrl"] = sys.argv[1]
json.dump(d, open(".devnet/localnet/deployment.json", "w"), indent=2)
PY

echo; echo "── preflight on the clone"
RPC_URL="$URL" npx tsx "scripts/devnet/$RUN.ts" --preflight
echo; echo "── $RUN.ts"
RPC_URL="$URL" COMMIT_SECS=${COMMIT_SECS:-60} REVEAL_SECS=${REVEAL_SECS:-40} npx tsx "scripts/devnet/$RUN.ts"
echo "NPCS REHEARSAL OK ($RUN)"
