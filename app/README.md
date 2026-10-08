# app/ — the pilot UI (#144, SRS 08 §7)

Next.js 16 (App Router). Every page reads the cluster at request time
through `@solana/kit`; accounts and events are decoded with the programs' own
IDLs (`idl/`, synced from `anchor build` with `npm run sync-idl`). There is no
backend and no cache between a page and the programs, and the server never
signs anything: participant writes are built in the browser and signed by
the participant's wallet (Wallet Standard: Phantom, Backpack, Squads).

All NPCS views carry the `PILOT-DEMO-ASSET` label.

| Screen | Route | What it shows | Where it works today |
|---|---|---|---|
| Session | `/sessions/[address]` | phase timeline and countdowns; the order book, sealed (commitment hash) until revealed; uniform-price clearing as demand/supply step curves against the reserve and the mark-engine NAV (they draw in on load); pro-rata allocations; fills and their DvP settlement; the mark-engine print; the rails `CashSession` totals. **Commit a sealed bid** (escrow + bond in payment-rails) and **reveal** it with a connected wallet; the preimage stays in the browser (localStorage + a download) | local validator (the engines are not on devnet yet) |
| Portfolio | `/portfolio?wallet=` | NPCS balance and frozen/thawed state, gate admission, the CG-P3 lot ledger with a seasoning countdown per lot, queue-claim requests, liquidity-facility positions, orders across sessions | devnet (NPCS holder H) and local |
| Gate check | `/gate?wallet=` | the gate-rejection view: **simulates credit-gate's real `validate`** for the wallet (nothing is signed or sent) and shows the program's own `ValidateDecision`, a typed `GateReject` or program error, next to every input it read: block entry, SAS attestations (signer, expiry vs horizon, jurisdiction, KYC, accreditation), holder governor, holder link, token account state, plus the simulation logs | devnet and local |
| Dashboard | `/` | which programs are deployed on the cluster, the NPCS gate market, its mark feed, session count | both |

Not built yet (SRS 08 §7): `/admin`, `/data-room`, the private allowlist,
a ROFR decision screen, tranche positions.

## Run against devnet

```bash
npm ci
npm run dev                       # http://localhost:3000, devnet by default
# e.g. /gate?wallet=BJL7L58MX82DiGxtjcq5VbzWuLLZe26U6R8PCfdJ2EMh  (admitted)
#      /gate?wallet=<any other wallet>                             (AttestationInvalid)
```

Configuration is in [`.env.example`](.env.example).

## Run the full session flow locally

`scripts/localnet.sh` gives the session page something real to show:

1. the LiteSVM harness (`payment-rails/tests-svm/tests/ui_fixture.rs`)
   runs a 3-bidder session through the real `.so` files at wall-clock time,
   to `Printed` (clear 0.95 against a 1.00 NAV), and opens a second session
   with four sealed orders;
2. `solana-test-validator` starts with every program at its real id and
   those accounts loaded. Throwaway demo wallets are in `.localnet/keys/`
   (import one into Phantom/Backpack to commit and reveal from the page).

```bash
# once: anchor build at the repo root and in payment-rails/
npm run localnet                  # COMMIT_SECS / REVEAL_SECS set the windows
NEXT_PUBLIC_CLUSTER=localnet NEXT_PUBLIC_NPCS_MINT=<printed mint> npm run dev
```

`npm run localnet:e2e` (validator started with `COMMIT_SECS=150
REVEAL_SECS=45`) checks the page's write path end to end: a new bidder
commits with `lib/tx.ts` `commitBidIx` (the session holds the page's
commitment hash byte for byte), all five orders reveal with `revealIx`, the
payment-rails crank's phase runner (#149) clears, allocates, settles both
fills DvP through the rails and the gate, and finishes; the session page's
loader then reads `Printed` at 0.945 with the print recorded.

## Tests

```bash
npm run lint && npm run typecheck && npm test && npm run build
```

`npm test` (vitest) decodes **real devnet bytes**: the NPCS gate market,
holder H's lot ledger, link and SAS attestations, and the `ValidateDecision`
the devnet gate emitted for a wallet with no attestations
(`test/fixtures/devnet-accounts.json`, refreshed with
`npx tsx --tsconfig tsconfig.json test/fetch-fixtures.mts`). It also pins the
clearing curves, the commitment hash, unit parsing and, when `anchor build`
output is present, that `idl/` matches it.
