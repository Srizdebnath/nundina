# payment-rails crank

This package holds the off-chain half of the rails (PHASES.md R4): the
**settlement and refund crank**, the **reconciliation ledger**, and the
**evidence export** for cash events. It uses TypeScript with `@solana/kit` v8,
following the same conventions as `token/`.

## The guarantee: the crank is optional

Nothing depends on this crank staying alive. The on-chain program decides
whether each step is done, not crank memory:

| Step | "Done" means | So a restarted or competing crank… |
|---|---|---|
| Settle fill *i* | Receipt PDA `["fill", session, i]` exists | skips it. A duplicate that lands anyway fails with *already in use* and counts as done |
| Refund an escrow | The escrow account is gone | skips it. A concurrent refund counts as done |

Refunds are **permissionless** on-chain, so any participant's wallet can run
`refunds`; no crank key is needed. `npm run drill` proves this on a real validator.

## Commands

```bash
npm install
npm run crank -- refunds   --session <addr>                    # any wallet
npm run crank -- settle    --session <addr> --plan fills.json [--max-sends N]
npm run crank -- finish    --session <addr>                    # operator
npm run crank -- reconcile --session <addr>                    # or --facility <addr>
npm run crank -- export    --out evidence/ [--limit N]       # signed by EVIDENCE_KEYPAIR (or KEYPAIR)
npm run crank -- keeper …  /  seasoning …                    # see § Bonded keepers below
npm run crank -- phases    --session <auction-engine session>  # phase-deadline steps (#149), any wallet
npm run crank -- distribute --pool <tranche pool> --cash N     # one coupon period from KEYPAIR's USDC (#141)
npm run crank -- run       --config runner.json [--once]       # the crank-runner: every duty on a schedule (#141)
npm run crank -- export    --session <auction session> --out evidence/s1   # one session, all four programs + snapshot (#141)
npm run crank -- verify-export --dir evidence/ [--signer <addr>]   # keyless, offline
```

`fills.json` is the auction engine's allocation:
`[{ "index": 0, "buyer": "<wallet>", "seller": "<wallet>", "qty": "1000000", "price": "98123456" }, …]`.

| Env | Default | Meaning |
|---|---|---|
| `RPC_URL` | `http://127.0.0.1:8899` | JSON-RPC |
| `WS_URL` | derived (port + 1) | subscriptions, used for confirmation |
| `KEYPAIR` | `~/.config/solana/id.json` | payer; the caller for refunds, the operator for settle and finish |
| `EVIDENCE_KEYPAIR` | `KEYPAIR` | signs `export` bundles (#127). Use a key of its own, published to auditors, not the operator's |
| `PRIORITY_FEE_MIN` / `_MAX` | `0` / `50000` | µlamports per CU; the p75 of recent fees for the program is clamped to these |
| `MAX_ATTEMPTS` / `RETRY_BASE_MS` | `5` / `500` | retries for **transient** failures, with exponential backoff and a fresh blockhash each time |
| `CRANK_KILL_FILE` | unset | `run` pauses phases, seasoning and distribution while this file exists; refunds keep running (never pausable). Checked once per tick (the AC-11 kill drill; liveness never depends on the runner) |

The exit code is 1 if any action fails or a reconciliation isn't clean.

## How it works

- **`src/idl.ts`** encodes instructions and decodes accounts and events
  straight from `idl/payment_rails.json`. After any program change, re-sync
  it with `npm run sync-idl` following `anchor build`. The account roles
  (writable and signer) come from the IDL, so the client can't get them wrong.
- **`src/send.ts`** sets a compute-unit limit and a priority fee. Only
  transient errors (expired blockhash, network, 429) are retried. Program
  errors come back typed (`code`, `name`, `alreadyInUse`) and are never
  retried blindly.
- **`src/rules.ts`** mirrors the on-chain refund rules, so the crank only
  sends refunds that will succeed. `tests/rules.spec.ts` pins it to the same
  truth table as the Rust tests.
- **`src/ledger.ts`** reconciles:
  - per session: Σ escrow cash, bond and asset against the session totals;
    each vault holding at least what its escrow records (any surplus is
    flagged as donated dust); receipts against the settled counters; each
    receipt's split summing to its gross;
  - per facility: `vault = funded − outstanding + interest`.
- **`src/evidence.ts`** walks the program's signature history and writes the
  decoded events to `events.jsonl`, plus a `manifest.json` with counts, the
  slot range and a **SHA-256** of the file. An event only counts if it was
  emitted inside the rails program's own call frame, so a program it calls
  can't inject look-alike events. The manifest is **signed** (#127): an
  ed25519 `attestation` by the exporting key over a domain-separated,
  key-sorted (canonical) JSON of the manifest, which carries the file's
  SHA-256. `verify-export` re-checks the hash, the event count and the
  signature, optionally against the signer the auditor was told to trust;
  it needs no key and no RPC.

## Bonded keepers and the seasoning crank (#127)

doc 06 §8: bonded keepers with a permissionless fallback. On-chain in
`payment-rails` (`instructions/keeper.rs`), driven from here:

```bash
KEYPAIR=<keeper.json>   npm run crank -- keeper register --bond 50000000 --token <keeper usdc>
KEYPAIR=<operator.json> npm run crank -- keeper assign --session <addr> --keeper <keeper wallet>
KEYPAIR=<anyone.json>   npm run crank -- refunds --session <addr> --escalate-to <their usdc>
KEYPAIR=<anyone.json>   npm run crank -- keeper release --session <addr>
KEYPAIR=<keeper.json>   npm run crank -- keeper withdraw --token <keeper usdc>
npm run crank -- seasoning --mint <gated mint>      # credit-gate merge_expired, any wallet
```

- The keeper posts a USDC bond (≥ `KeeperPolicy.min_bond`) into a vault its
  `Keeper` PDA owns. The operator assigns it per session, snapshotting
  `due_at = settle_deadline + sla_seconds` and the reward.
- Inside the SLA the keeper refunds with the ordinary `refunds`. Past it,
  `refunds --escalate-to` runs `refund_escalated` / `refund_ask_escalated`:
  the same refund (one shared body), and the caller is paid the reward
  from the keeper's bond, capped by what the bond holds (never blocking the
  refund). The escrow owner may escalate its own refund.
- The bond can't leave while the keeper holds a live assignment;
  `release` (anyone) closes an assignment once the session has no escrow.
- `seasoning` lists the gated mint's `HolderLots` ledgers and sends
  `merge_expired` for every one holding a lot past `restriction_expiry`
  (layout pinned by credit-gate's `layout_read_by_the_seasoning_crank`).

The kill-the-crank drill exercises the keeper path on a real validator: a
bonded keeper is assigned, dies with the crank, and the participant who
refunds after the SLA is paid from its bond (38 checks).

Still not built, because the programs they would drive don't exist:

| SRS 08 §6 | Waits on |
|---|---|
| Phase-deadline cranks (Commit → Reveal → Clearing …) | `auction-engine` (#136). The rails' own deadline duty (refunds) is the keeper SLA above |
| Distribution cranks | `tranche-engine` (#140); `distribute_coupon` is called by the distributor |
| Session bundles with marks + model versions (AC-12) | `mark-engine` (#137) |

The R5 devnet run settled Circle devnet USDC against a **manifest mint, not
live NPCS** (`../PHASES.md` R5). R5 is met for the cash leg; the live-NPCS
DvP stays open until the gate can thaw an escrow vault.

## Tests

```bash
npm run typecheck    # tsc --strict, noUncheckedIndexedAccess
npm test             # unit tests: codec vs IDL, refund rules, fees, error classes, evidence attribution, evidence signing
npm run drill        # kill-the-crank drill on a fresh solana-test-validator
```

The drill needs `anchor build` in `payment-rails/` and `solana-test-validator`
on the PATH. It deploys the built program as upgradeable, then:
1. The crank settles fill #0 and **dies**. It restarts, skips #0, settles #1,
   and dies for good without ever calling `finish_session`.
2. Reconciliation is clean **mid-session**, with 5 live escrows.
3. Before the deadline, a participant can refund only the non-revealer (bond
   → reserve). After it, that participant refunds **everyone** using their
   own wallet, and a second run finds nothing left.
4. Exact per-party balances hold, and USDC and asset are conserved. The
   session and facility ledgers are clean, and the evidence SHA-256 and
   its signature (by a key of its own) re-verify.
5. Finally, the CLI runs `reconcile`, `refunds`, `export` and
   `verify-export` against the final state.

## Phase-deadline crank (#149)

`phases --session <addr>` drives an **auction-engine** session: it reads the
session (decoded with `idl/auction_engine.json` through the same IDL codec,
`makeCodec`), asks the pure planner `planPhase` for the next permissionless
step, sends it, and repeats until nothing is due:

| Phase | Step sent |
|---|---|
| Commit / Reveal, `now ≥ reveal_end` | `clear` (fails the session if too few reveals or no cross) |
| Allocating | `allocate` (8 fills per tx) |
| IssuerRofr, `now ≥ reveal_end + rofr_window` | `close_rofr` |
| Settling | `settle` for the first unsettled fill (rails `settle_fill` + gate accounts forwarded), then `finish` (rails `finish_session` + print into mark-engine) |
| any live phase, `now ≥ settle_deadline` | `expire` |

It keeps no state: a restarted or competing crank re-reads the session.
Refunds after `finish` / `expire` are the rails' own (`refunds`). Seasoning
merges are `seasoning --mint` (above); bonded keepers cover the SLA.
`tests/phases.spec.ts` pins the planner to the program's guards and the
decoder to the program's IDL. **Run on a validator** with the real programs
by the UI's local demo (`app/scripts/localnet-e2e.mts`, #144): clear,
allocate, two settles and finish to `Printed`. That run found the `settle`
step is 1,684 bytes as a plain transaction (rails `settle_fill` + gate
`settle_check` ≈ 30 accounts; LiteSVM has no packet limit), so settle steps
go through an address lookup table the crank creates and extends on demand
(`src/alt.ts`, `tests/alt.spec.ts`). The table costs rent, paid by the crank
wallet; it is not closed automatically.

## Crank-runner, distribution and session evidence (#141)

`run --config runner.json` is the hosted crank-runner of SRS 08 §6. Each
tick (every `intervalSeconds`; `--once` for cron) it:

1. finds every auction-engine session on chain and runs the phase crank on
   the live ones;
2. refunds every finished or failed session whose rails `CashSession` still
   has open escrows (bonds back to their owners);
3. runs `seasoning` for each mint in `seasoningMints`;
4. for each pool in `distribution`, pays the next coupon period
   (`cashPerPeriod` µUSDC from the runner wallet) once `minIntervalSeconds`
   have passed since the last one. If the wallet can't pay, the period is
   skipped and reported.

```json
{ "intervalSeconds": 60, "seasoningMints": ["<NPCS>"],
  "distribution": [{ "pool": "<tranche pool>", "cashPerPeriod": "1500000", "minIntervalSeconds": 604800 }] }
```

`distribute --pool --cash` is step 4 on its own: tranche-engine's
permissionless `distribute(period, cash)` with the rails
`DistributeCoupon` accounts forwarded (coupon receipt for
`rails_period_id(pool_id, period)`, payees' registered destinations, the
tranche signer's `EngineGrant`). The period is the pool's own counter.

`export --session` writes one session's bundle: every event of
auction-engine, payment-rails, credit-gate and mark-engine in every
transaction that touched the session or its rails session, attributed by
invoke stack (the codec now reads `u128` and `Option`); `snapshot.json` with
the session, the CashSession, the mark feed (NAV, `model_version`,
`inputs_hash`, state) and the print; and a manifest carrying both
SHA-256s, signed. `verify-export` checks the snapshot hash as well.

Verified on a local validator with the real programs (`app/scripts/localnet.sh`).
One `run --once` tick refunded 9 escrows across two printed sessions,
merged 2 seasoned ledgers and paid a coupon period. A session bundle came out at
16 transactions and 39 events, with both fills proven under AE-P8.
Unit tests: `tests/runner.spec.ts`.

## Live-NPCS DvP (R5 on the real mint, #145 / #172)

`scripts/devnet/npcs-dvp.ts` records one DvP where the asset leg is the
devnet NPCS mint `6vt1…VKL4`. Holder H sells 1.0 NPCS, and buyers X (0.6) and
Y (0.4) buy at 0.98:
1. Each buyer is SAS-attested, passes `validate`, and gets an NPCS account
   that Token ACL thaws permissionlessly; credit-gate decides.
2. The escrow's asset vault is a program PDA, never an investor, so the
   Token ACL MintConfig authority thaws it with the authority `Thaw`
   (instruction 4, from the upstream client).
3. Each fill's `settle_fill` CPIs credit-gate `settle_check`.
4. The run finishes with refunds, reconciliation and a signed evidence
   bundle, plus a step record with links.

```bash
RPC_URL=https://api.devnet.solana.com npx tsx scripts/devnet/npcs-dvp.ts --preflight   # read-only
RPC_URL=https://api.devnet.solana.com npx tsx scripts/devnet/npcs-dvp.ts
bash scripts/devnet/rehearse-npcs.sh      # the same run on a local clone of devnet
```

`--preflight` lists what still blocks the run: the approved payment-rails
upgrade (Squads proposal #2; its buffer closes when executed), H's lot
seasoning, the keys, and the operator's USDC and SOL. `rehearse-npcs.sh`
clones every devnet account the run touches and loads payment-rails from
that approved buffer (sha256 `b7681924…fbec`, the exact build devnet will
run). It patches only H's seasoning expiry and the operator's USDC balance
(`npcs-clone.ts`), then runs `npcs-dvp.ts` unchanged. It passes: both fills
settle through the gate, reconciliation is clean, and 2/2 fills are proven
under AE-P8.