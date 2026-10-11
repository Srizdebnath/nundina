# services/ — off-chain services (SRS 08 §6, #141)

A Node 24 / TypeScript package (`@solana/kit`). Decoding uses the programs'
own IDLs: one copy lives in `client/src/idl/`, and `src/codec.ts` is the app's decoder
verbatim (a test enforces it; after changing `client/src/lib/borsh.ts`, run
`npm run sync-codec`). The cranks, the crank-runner and the
session evidence exporter live with the rails crank, in
[`payment-rails/crank/`](../payment-rails/crank/README.md).

| Service (SRS 08 §6) | Where | What it does | Verified against |
|---|---|---|---|
| **indexer** | `src/indexer.ts` · `schema.sql` · `npm run svc -- indexer` / `webhook` | Decodes every event of the 8 programs from transaction logs. Each event is attributed to the program on top of the invoke stack: auction-engine and payment-rails both emit `FillSettled` with the same discriminator. Events land in `events` and are projected into `sessions`, `bids`, `fills`, `prints`, `marks`, `mark_states`, `lot_events`, `gate_decisions`, `claims`, `advances`, `waterfall_lines` and `payouts`. Two feeds share one ingestion path: an RPC backfill with per-program cursors (also the gap-fill) and a Helius raw-transaction webhook receiver (`Authorization` = `WEBHOOK_SECRET`). Failed transactions are skipped. Re-ingesting is a no-op: every projection is a SET, phases only move forward, and state columns take the latest slot. | real transactions of a local demo run (`test/fixtures/localnet-txs.json`) in real Postgres (PGlite), including re-ingest in reverse order; **devnet**: credit-gate's history (holder H's `ValidateDecision`, its seasoning lot) |
| **api** | `src/api.ts` · `npm run svc -- api` | Read-only JSON over those tables: `/health`, `/sessions`, `/sessions/:s` (bids, fills with the rails cash split, print, events), `/holders/:w`, `/marks/:feed`, `/pools/:p/waterfall`, `/events`. There are no write routes: user actions are wallet-signed transactions built client-side (`client/src/lib/tx.ts`). | test suite (HTTP); local demo |
| **nav-publisher** | `src/nav.ts` · `src/nav-publisher.ts` · `npm run svc -- nav` | §3.2. Reads the treasury (`treasury:<npcs-treasury PDA>` or `sleeve:<USDC account>`), the supply, the feed and the Clock in **one** `getMultipleAccounts` (one slot). It computes NAV with `credit-math`'s exact arithmetic (bigint, floors against the publisher, pinned to the crate's vectors) and hashes the canonical inputs document into `inputs_hash`. It then sends `mark-engine::publish_nav` signed by a publisher-set key and writes the inputs document as evidence. Weekly from cron or a systemd timer; `--dry-run` sends nothing. | **devnet dry run**: sleeve `B78W…NKAz` holds 1 USDC and supply is 1 NPCS, so NAV = 1.000000; NAV #1 was then published to the devnet NPCS feed. **Local**: published 0.999999877854 (130 USDC / 130 NPCS, less 3,852 s of the 10 bps/yr fee) |
| **treasury-agent** | `src/treasury.ts` · `npm run svc -- treasury` | Runbook-driven, holds no money key (deposits and withdrawals stay manual, `token/runbook.md` § Treasury). `status` is the weekly I1 check: supply == USDC × 10^(dec diff). `reconcile` sends npcs-treasury's permissionless `reconcile`, which evaluates I1 on chain and emits `Reconciled { backed }`. | **devnet**: `status` on the manual sleeve → **BACKED** |
| **crank-runner** | `payment-rails/crank` `run --config` | Each tick: drive every live auction session (phases, #149), refund finished or failed sessions with open escrows, run credit-gate `merge_expired` for configured mints, pay due tranche periods (`distribute`). Kill switch: `CRANK_KILL_FILE` (pauses all but refunds). | local validator, real programs: one tick refunded 9 escrows, merged 2 seasoned ledgers, paid a coupon period |
| **evidence-exporter** | `payment-rails/crank` `export --session` | One auction session: every event of auction-engine, payment-rails, credit-gate and mark-engine in every transaction touching the session or its rails session; a state snapshot (session, CashSession, feed NAV / model_version / inputs_hash, print); a manifest with both SHA-256s, ed25519-signed. `verify-export` checks all of it offline. | local: 16 txs, 39 events, 2/2 fills **proven** (AE-P8), verifies |

## Not done (honest list)

- **Hosting.** Nothing runs continuously yet. No Supabase project or Helius
  webhook is provisioned (neither has keys in this repo), and no VPS or
  systemd unit is deployed. Everything runs from the CLI against any RPC.
  `DATABASE_URL=pglite:<dir>` gives a working local database without
  Postgres.
- **§3.2 step 5** (a SAS attestation of each NAV): `publish_nav` takes no
  attestation account, so nothing on chain could verify one. The control
  is the publisher-set signature plus the published `inputs_hash`.
- **Devnet coverage** follows the deployments (`DEPLOYMENTS.md`):
  credit-gate, payment-rails, auction-engine, mark-engine, queue-claim,
  tranche-engine and npcs-treasury are on devnet (liquidity-facility is
  not). No session, claim or pool has run there yet, so sessions, claims
  and pools are indexed from the local validator; devnet marks index from
  the NPCS feed (NAV #1). Devnet payment-rails still
  runs the v0.1.0 build: its R5 history indexes (4 settled fills, payouts,
  facility and session events), but its 26 `EscrowEvent`s predate #89's
  `unit` field. Events that don't fit the IDL are stored raw
  (`data.undecoded`) and not projected; nothing is dropped.
- Logs over ~10 KB are truncated by RPC nodes; events after the cut are not
  indexed (no instruction here comes close).

## Run

```bash
npm ci
npm run typecheck && npm test

# indexer + api on a local database (or DATABASE_URL=postgres://…)
RPC_URL=https://api.devnet.solana.com npm run svc -- indexer --programs credit-gate,payment-rails
npm run svc -- api --port 8788           # GET /health, /sessions, /holders/<wallet>, …
WEBHOOK_SECRET=… npm run svc -- webhook --port 8787

# weekly NAV (PUBLISHER = a key in the feed's publisher set)
RPC_URL=… PUBLISHER=~/.config/nundina/nav-publisher.json \
  npm run svc -- nav --mint <NPCS> --source sleeve:<USDC account> --out evidence/nav
# cron:  0 12 * * 1  cd /srv/nundina/services && npm run --silent svc -- nav … >> nav.log 2>&1

# treasury reconciliation
npm run svc -- treasury status --source sleeve:B78WTdsY4MYhAvT757xtdb6KZ61pkBTPzKCtohK3NKAz --mint 6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4
npm run svc -- treasury reconcile --source treasury:<npcs-treasury PDA>
```

Local demo end to end (validator with the real programs and demo state):
`client/scripts/localnet.sh`, then `app` `npm run localnet:e2e`, then the
crank-runner and the services as above with `RPC_URL=http://127.0.0.1:8899`.
`test/capture-localnet.ts` refreshes the indexer's test transactions from
such a run.
