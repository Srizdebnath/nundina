# client/ — nundina.xyz: site + dApp

Next.js 16 (App Router, `src/`, TypeScript, Tailwind v4, Cache Components). One
deployment serves the marketing site and the live, chain-reading dApp under
`/app`. Vercel builds this folder on every push to `main`.

```bash
npm ci
npm run dev        # http://localhost:3000, devnet by default
npm run lint && npm run typecheck && npm test && npm run build
```

## Routes

| Route | What it is |
|---|---|
| `/` · `/protocol` · `/architecture` · `/security` | marketing pages |
| `/deployments` · `/status` | program registry and roadmap; deployment status is read live from the cluster |
| `/docs` · `/docs/[slug]` | the 15-chapter documentation (`src/content/docs/`) |
| `/legal/terms` · `/legal/privacy` | pilot notices |
| `/app` | dApp overview: which programs are deployed, the NPCS gate market, its mark feed |
| `/app/sessions` · `/app/sessions/[address]` | auction sessions: phase timeline, sealed → revealed book, clearing curves vs reserve and NAV, fills settled DvP, print; **commit / reveal / commit_ask / crank** from a connected wallet; issuer ROFR |
| `/app/portfolio?wallet=` | NPCS balance and freeze state, gate admission, lot ledger with seasoning countdown, claims, advances, orders, tranche positions |
| `/app/gate?wallet=` | simulates credit-gate's real `validate` (nothing is signed or sent) and shows the program's own decision or typed `GateReject`, with every input it read |
| `/app/queue` · `/app/facility` · `/app/tranches` · `/app/treasury` | redemption requests, advances, pools and NPCS mint/redeem flows |
| `/app/data-room` | every mark-engine print in full; allowlisted viewers only |
| `/app/admin` | the authority map read from the chain, and admin actions signed by the holding key (or built as a Squads proposal) |
| `/api/access` | wallet-signed sign-in for the gated pages |

Every `/app` page reads the cluster at request time through `@solana/kit` and
decodes accounts with the programs' own IDLs (`src/idl/`, synced from
`anchor build` with `npm run sync-idl`). The server never signs: writes are
built in the browser and signed by the participant's wallet.

## Configuration

Deployment addresses are **only** in the environment, never in source:
`.env` (committed: public program ids for devnet) is read by every build,
and `.env.local` (git-ignored) overrides it locally. [`.env.example`](.env.example)
lists every variable. Program ids do not change on upgrade, so `.env` changes
only after a deploy to a new address. If a variable is missing, the app shows
a banner naming it rather than reading the wrong accounts.

Server-only settings (`NUNDINA_ACCESS`, `NUNDINA_ACCESS_SECRET`,
`NUNDINA_ALLOWLIST`, `NUNDINA_ACCESS_LOG`) are set in the Vercel project, not
in a committed file. Without `NUNDINA_ACCESS_SECRET` (≥ 32 chars) nobody can
sign in to `/app/admin` and `/app/data-room`.

## Wallets

Wallet Standard (`src/lib/wallet.tsx`). **Phantom is the primary wallet**:
listed first, and offered as the install link or, on mobile, as an
"open in Phantom" deep link when no wallet is detected. Every other
Solana wallet that registers through the standard (Solflare, Backpack,
Glow, Squads, …) is listed after it. The last wallet used reconnects
silently; account switches inside the wallet are followed.

## Private allowlist

`NUNDINA_ACCESS=allowlist` puts every page behind a sign-in (SRS 08 §7). The
server issues a challenge (wallet, cluster, time, an HMAC'd nonce), the wallet
signs it, and the server admits the wallet if it is on `NUNDINA_ALLOWLIST` or
credit-gate's own `validate`, simulated on the NPCS market, approves it. The
answer is an HMAC'd httpOnly cookie for 12 h. Without the variable only
`/app/data-room` and `/app/admin` ask for a sign-in. Every sign-in, refusal
and gated view is one JSON line (`src/lib/access-log.ts`).

```bash
NUNDINA_ACCESS=allowlist NUNDINA_ACCESS_SECRET=$(openssl rand -hex 32) npm run start -- -p 3123 &
BASE=http://127.0.0.1:3123 HOLDER=~/.config/nundina/npcs-devnet/holder-h.json npm run access:e2e
```

## Run the full session flow locally

`scripts/localnet.sh` runs a 3-bidder session through the real `.so` files in
the LiteSVM harness (`payment-rails/tests-svm/tests/ui_fixture.rs`), then
starts `solana-test-validator` with every program at its real id and those
accounts loaded. Throwaway demo wallets are written to `.localnet/keys/`.

```bash
# once: anchor build at the repo root and in payment-rails/
npm run localnet                  # COMMIT_SECS / REVEAL_SECS set the windows
NEXT_PUBLIC_CLUSTER=localnet NEXT_PUBLIC_NPCS_MINT=<printed mint> npm run dev
```

`npm run localnet:e2e` (validator started with `COMMIT_SECS=150
REVEAL_SECS=45`) checks the write path end to end: commit with `commitBidIx`,
reveal with `revealIx`, the crank clears, allocates, settles both fills DvP
through the rails and the gate, and finishes.

## Tests

`npm test` (vitest) decodes **real devnet bytes**: the NPCS gate market,
holder H's lot ledger, link and SAS attestations, and a `ValidateDecision`
(`test/fixtures/devnet-accounts.json`, refreshed with
`npx tsx --tsconfig tsconfig.json test/fetch-fixtures.mts`). It also pins the
clearing curves, the commitment hash, unit parsing, the tx builders and, when
`anchor build` output is present, that `src/idl/` matches it.

## Layout

`src/app/(site)` marketing pages · `src/app/app` dApp · `src/components/{layout,landing,ui,page,docs}`
site components · `src/components/app` dApp components · `src/lib` chain reads, codecs, tx builders,
wallet · `src/lib/site.ts` site routes · `src/lib/data.ts` every landing-page number with its source.
Content rules: devnet facts only, cited market data, no APY/TVL/audit claims.
