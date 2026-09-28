#!/usr/bin/env bash
# Deploy payment-rails at its fixed program id, verify the on-chain bytes,
# and record the deployment (devnet/testnet/mainnet; not local) in
# payment-rails/DEPLOYMENTS.md.
#
#   CLUSTER=devnet   AUTHORITY=~/.config/solana/id.json  bash deploy.sh
#   CLUSTER=http://127.0.0.1:8899                         bash deploy.sh   # rehearsal
#   CLUSTER=devnet RPC_URL=https://devnet.helius-rpc.com/?api-key=… bash deploy.sh
#   RPC_URL=… bash deploy.sh --dry-run    # print (RPC, genesis, cluster) and exit
#   bash deploy.sh --help                 # this text; touches nothing
#
# Arguments are parsed strictly: anything other than --dry-run / --help
# aborts before any network access (#55). DRY_RUN, UPGRADE, USE_RPC and
# I_MEAN_MAINNET accept only 0 or 1.
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
SELF=$(realpath "$0")
cd "$(dirname "$0")/../../.."   # → payment-rails/

die() { echo "✗ $*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$SELF"; }

DRY_RUN=${DRY_RUN:-0}
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument '$arg' (accepted: --dry-run, --help) — nothing was done" ;;
  esac
done
for v in DRY_RUN UPGRADE USE_RPC I_MEAN_MAINNET; do
  case "${!v:-0}" in 0|1) ;; *) die "$v must be 0 or 1, got '${!v}' — nothing was done" ;; esac
done

EXPECTED_ID=GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn
REQUESTED=${CLUSTER:-}   # endpoint shorthand/URL, or a moniker to assert
RPC=${RPC_URL:-${REQUESTED:-devnet}}
RPC_SHOWN=${RPC%%\?*}   # for output: a query string may carry an API key
AUTHORITY=${AUTHORITY:-$HOME/.config/solana/id.json}
SO=target/deploy/payment_rails.so
PROGRAM_KEY=keys/payment_rails-keypair.json
PRICE=${PRIORITY_FEE:-20000}   # µlamports / CU for deploy transactions
SEND_VIA=(--use-rpc); [ "${USE_RPC:-1}" = 0 ] && SEND_VIA=()

# The solana CLI embeds the full request URL in its errors, so anything it
# prints can carry an `?api-key=`. Everything shown passes through redact.
redact() {
  local s
  while IFS= read -r s || [ -n "$s" ]; do
    s=${s//"$RPC"/"$RPC_SHOWN"}
    printf '%s\n' "$s" | sed -E 's/([?&][A-Za-z_-]*(key|token|secret)[A-Za-z_-]*=)[^&[:space:]")]*/\1<redacted>/Ig'
  done
}
sol() { solana -u "$RPC" "$@" 2> >(redact >&2); }

# Endpoint URL for raw JSON-RPC (the CLI accepts monikers too).
case "$RPC" in
  d|devnet) RPC_HTTP=https://api.devnet.solana.com ;;
  m|mainnet-beta) RPC_HTTP=https://api.mainnet-beta.solana.com ;;
  t|testnet) RPC_HTTP=https://api.testnet.solana.com ;;
  l|localhost) RPC_HTTP=http://127.0.0.1:8899 ;;
  *) RPC_HTTP=$RPC ;;
esac

# Does an account exist? Prints `present` or `absent`. An RPC error, a
# timeout or a garbled reply is neither: retried, then fatal. Treating an
# error as "absent" would skip the UPGRADE=1 guard and mislabel the
# registry row (#55).
account_state() {
  local body reply
  body='{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["'"$1"'",{"encoding":"base64","dataSlice":{"offset":0,"length":0},"commitment":"confirmed"}]}'
  for delay in 2 4 8 0; do
    reply=$(curl -sS --max-time 20 -H 'Content-Type: application/json' -d "$body" "$RPC_HTTP" 2>/dev/null | tr -d ' \n\t' || true)
    case "$reply" in
      *'"error"'*) ;;
      *'"value":null'*) echo absent; return 0 ;;
      *'"value":{'*) echo present; return 0 ;;
    esac
    [ "$delay" = 0 ] || sleep "$delay"
  done
  die "cannot tell whether $1 exists at $RPC_SHOWN (RPC error, 4 attempts) — refusing to guess"
}

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

# The program id is fixed; the keypair is only needed for a FIRST deploy at
# that id. Upgrading an existing program needs only the upgrade authority,
# so a lost program keypair never blocks an upgrade (#57).
PROGRAM_ID=$EXPECTED_ID
HAVE_KEY=0
if [ -f "$PROGRAM_KEY" ]; then
  KEY_ID=$(solana-keygen pubkey "$PROGRAM_KEY")
  [ "$KEY_ID" = "$EXPECTED_ID" ] || die "program keypair is $KEY_ID, expected $EXPECTED_ID"
  HAVE_KEY=1
fi
[ -f "$SO" ] || die "missing $SO — run 'anchor build' first"
AUTH_PK=$(solana-keygen pubkey "$AUTHORITY")
LEN=$(stat -c %s "$SO")
BUILD_SHA=$(sha256sum "$SO" | cut -d' ' -f1)
echo "program   $PROGRAM_ID"
echo "authority $AUTH_PK"
echo "build     $BUILD_SHA ($LEN bytes)"

# The ~850 KB dump fails often on the public devnet RPC: retry with backoff.
verify_dump() {
  local tmp got delay; tmp=$(mktemp)
  for delay in 2 4 8 16 0; do
    if sol program dump "$PROGRAM_ID" "$tmp" >/dev/null; then
      # ProgramData keeps its allocation after an upgrade to a smaller build,
      # so the dump is the build followed by zero padding. Hash the build's
      # length and require the rest to be zeros; otherwise a shrinking
      # upgrade reads as a mismatch.
      local pad; pad=$(tail -c +$((LEN + 1)) "$tmp" | tr -d '\0' | wc -c)
      if [ "$pad" = 0 ]; then got=$(head -c "$LEN" "$tmp" | sha256sum | cut -d' ' -f1)
      else got="non-zero-bytes-past-build($(sha256sum "$tmp" | cut -d' ' -f1))"; fi
      rm -f "$tmp"; echo "$got"; return 0
    fi
    [ "$delay" = 0 ] || { echo "  program dump failed; retrying in ${delay}s" >&2; sleep "$delay"; }
  done
  rm -f "$tmp"; die "program dump failed 5 times at $RPC_SHOWN"
}

# Assigned first: a `die` inside $(…) only exits the subshell, and a bare
# `[ "$(…)" = present ]` would then fall through to MODE=deploy.
PROGRAM_STATE=$(account_state "$PROGRAM_ID")
if [ "$PROGRAM_STATE" = present ]; then
  ONCHAIN=$(verify_dump)
  if [ "$ONCHAIN" = "$BUILD_SHA" ]; then
    echo "✓ already deployed with this exact build — nothing to do"
    exit 0
  fi
  [ "${UPGRADE:-0}" = 1 ] || die "program exists with a different build ($ONCHAIN); re-run with UPGRADE=1 to upgrade"
  CURRENT_AUTH=$(sol program show "$PROGRAM_ID" | awk '/^Authority:/ {print $2}')
  [ -n "$CURRENT_AUTH" ] || die "could not read the upgrade authority of $PROGRAM_ID" 
  [ "$CURRENT_AUTH" = "$AUTH_PK" ] || die "upgrade authority is $CURRENT_AUTH, not $AUTH_PK"
  MODE=upgrade
else
  MODE=deploy
  if [ "$HAVE_KEY" != 1 ]; then
    cat >&2 <<EOF
✗ $PROGRAM_ID is not deployed on $CLUSTER, and a FIRST deploy at this id
  needs payment-rails/$PROGRAM_KEY (the program keypair). It is gitignored,
  never committed, and CANNOT be regenerated: a new keypair is a new id.

  Restore it from the offline backup. Who holds the backups is recorded in
  the team's private ops notes (README.md § Keys says what to back up, not
  where), then check it:
    mkdir -p keys && cp <backup>/payment_rails-keypair.json keys/
    solana-keygen pubkey keys/payment_rails-keypair.json   # must print $EXPECTED_ID

  Upgrading the live program does NOT need this key, only the upgrade
  authority. Only verifying it needs no key at all:
    bash crank/scripts/devnet/verify.sh
EOF
    exit 1
  fi
fi
# First deploy signs with the program keypair. An upgrade addresses the
# program by its bare id: the loader checks the upgrade authority only.
PROGRAM_ARG=$PROGRAM_ID; [ "$MODE" = deploy ] && PROGRAM_ARG=$PROGRAM_KEY

# Cost check: ProgramData rent (+ an equal-size buffer during the write,
# refunded at the end) + fees margin.
RENT=$(sol rent $((LEN + 45)) --lamports | awk '{print $NF == "lamports" ? $(NF-1) : $NF}' | tr -dc 0-9)
NEED=$((RENT * 2 + 200000000))
BAL=$(sol balance "$AUTH_PK" --lamports | tr -dc 0-9)
echo "rent      $((RENT / 1000000)) mSOL · need ≈ $((NEED / 1000000)) mSOL · have $((BAL / 1000000)) mSOL"
if [ "$BAL" -lt "$NEED" ]; then
  die "authority needs ≈ $(( (NEED - BAL) / 1000000 + 1 )) more mSOL — fund $AUTH_PK (devnet: https://faucet.solana.com) and re-run"
fi

# A failed write leaves a funded buffer owned by the authority. Name it and
# the exact reclaim command, rather than leaving the operator to find it.
# (Resuming needs the buffer's ephemeral keypair — the seed phrase solana
# prints above; re-running this script simply writes a fresh buffer.)
stranded_buffers() {
  local bufs
  bufs=$(sol program show --buffers --buffer-authority "$AUTH_PK" 2>/dev/null \
    | awk 'length($1) >= 32 && length($1) <= 44 && $1 ~ /^[1-9A-HJ-NP-Za-km-z]+$/ {print $1 " (" $(NF-1) " SOL)"}') || true
  [ -n "$bufs" ] || return 0
  echo "✗ SOL is held in buffer(s) owned by $AUTH_PK:" >&2
  echo "$bufs" | sed 's/^/    /' >&2
  echo "  reclaim all: solana -u <rpc> program close --buffers --authority $AUTHORITY --recipient $AUTH_PK" >&2
}

echo "→ $MODE"
OUT=$(sol program deploy "$SO" \
  --program-id "$PROGRAM_ARG" \
  --upgrade-authority "$AUTHORITY" \
  --keypair "$AUTHORITY" \
  --max-len "$LEN" \
  --with-compute-unit-price "$PRICE" \
  --max-sign-attempts 50 \
  "${SEND_VIA[@]}" 2>&1) || { echo "$OUT" | redact >&2; stranded_buffers; die "deploy failed"; }
echo "$OUT" | redact
SIG=$(echo "$OUT" | awk '/^Signature:/ {print $2}')

ONCHAIN=$(verify_dump)
[ "$ONCHAIN" = "$BUILD_SHA" ] || die "on-chain dump $ONCHAIN != build $BUILD_SHA"
SHOW=$(sol program show "$PROGRAM_ID")
SLOT=$(echo "$SHOW" | awk '/Last Deployed In Slot:/ {print $NF}')
ONCHAIN_AUTH=$(echo "$SHOW" | awk '/^Authority:/ {print $2}')
[ "$ONCHAIN_AUTH" = "$AUTH_PK" ] || die "upgrade authority on-chain is $ONCHAIN_AUTH"
echo "✓ verified: dump SHA-256 == build, authority $ONCHAIN_AUTH, slot $SLOT"

case "$CLUSTER" in
  devnet|testnet|mainnet-beta)
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
