# services/ — off-chain services (SRS 08 §6)

**Status (2026-10): none of these is built.** This folder exists because the
P0 exit checklist (#4) named it; it holds this deferral record and nothing
else. P0 was closed with it outstanding, and #100 re-baselined the checklist
honestly instead of badging it complete. The substance is tracked in **#58**.

The one off-chain component that *does* exist is the payment-rails crank,
which also does reconciliation and evidence export:
[`payment-rails/crank/`](../payment-rails/crank/README.md).

| Service (SRS 08 §6) | What it must do | Built | Deferred to | Tracked |
|---|---|---|---|---|
| `nav-publisher` | Weekly NAV publish (doc 09 §3 I2): read the sleeve, compute gross/fee/published NAV with `credit-math`, post the mark | ❌ no cron, key or publisher set | P1 | #58 |
| `crank-runner` | Bonded keepers for settle / refund / finish, with the permissionless fallback (D6) | ⚠️ partial: `payment-rails/crank` runs settle / refund / finish idempotently as a CLI; no hosted runner, no bonding, no phase-deadline / seasoning / distribution cranks (#127) | P1 | #58 |
| `indexer` | Decode every program event into normalized tables (sessions, bids, fills, lots, marks, claims, advances, waterfall lines) | ❌ no DB, schema, webhook or backfill | P1 | #58 |
| `evidence-exporter` | AC-12 bundle: every event, hashed, attributable | ⚠️ partial: `crank export` writes JSONL + a SHA-256 manifest, ed25519-signed (#127), for payment-rails events only | P1 | #58 |

Providers named in the P0 checklist (Helius dev RPC/webhooks, Supabase dev
database) are **not provisioned**; no project ref, URL, schema or
`.env.example` exists in this repo. They are provisioned with the indexer, in
P1.
