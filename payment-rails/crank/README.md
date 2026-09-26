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
npm run crank -- export    --out evidence/ [--limit N]
```

`fills.json` is the auction engine's allocation:
`[{ "index": 0, "buyer": "<wallet>", "seller": "<wallet>", "qty": "1000000", "price": "98123456" }, …]`.

| Env | Default | Meaning |
|---|---|---|
| `RPC_URL` | `http://127.0.0.1:8899` | JSON-RPC |
| `WS_URL` | derived (port + 1) | subscriptions, used for confirmation |
| `KEYPAIR` | `~/.config/solana/id.json` | payer; the caller for refunds, the operator for settle and finish |
| `PRIORITY_FEE_MIN` / `_MAX` | `0` / `50000` | µlamports per CU; the p75 of recent fees for the program is clamped to these |
| `MAX_ATTEMPTS` / `RETRY_BASE_MS` | `5` / `500` | retries for **transient** failures, with exponential backoff and a fresh blockhash each time |

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
  can't inject look-alike events.

## Tests

```bash
npm run typecheck    # tsc --strict, noUncheckedIndexedAccess
npm test             # 16 unit tests: codec vs IDL, refund rules, fees, error classes, evidence attribution
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
   session and facility ledgers are clean, and the evidence SHA-256
   re-verifies.
5. Finally, the CLI runs `reconcile`, `refunds` and `export` against the
   final state.
