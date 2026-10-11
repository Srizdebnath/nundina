# NUNDINA — Status vs Architecture / SRS / Hackathon

> Audit snapshot: **2026-10-09**, after merges **#200** and **#201**.
> Local `main` is at `df5ff2a9d4519d4a219b118a8fcc5d54f2572b6e`, matching the
> existing `origin/main` tracking ref.

## Executive summary

NUNDINA has a substantial, real implementation. The core programs, Token ACL /
SAS compliance path, payment rails, local integration suites, crank, evidence
exporter, and UI are not placeholder business logic.

The headline pilot flow is not complete yet:

```text
live NPCS DvP → live sealed-bid auction → live queue-claim sale
```

The gate/compliance slice is real on devnet. The auction venue, mark feed,
queue, treasury binary, and tranche binary are deployed or initialized at the
program/state level. There is still no recorded live-devnet auction session,
live queue-claim sale, treasury-enforced mint path, facility deployment, or
mainnet pilot.

### Overall estimate

| Scope | Estimate | Meaning |
|---|---:|---|
| Core engineering implementation | **~60–70%** | Most program logic, tests, tooling, and UI paths exist. |
| Credible live-devnet hackathon demo | **~35–45%** | The gate is live; the headline auction/queue flow is not recorded live. |
| Strict SRS pilot | **0/12 complete** | The SRS requires all scenarios on mainnet-beta with real USDC and evidence. |
| Anchor programs with code/tests/IDL | **8/8** | Includes `npcs-treasury`, added beyond the original seven-program inventory. |
| Anchor programs deployed on devnet | **7/8** | `liquidity-facility` is not deployed. |
| Programs deployed on mainnet-beta | **0/8** | Mainnet deployment has not started. |

The percentages are scope-specific estimates. They do not mean that 60–70% of
the strict mainnet acceptance criteria are complete.

## Repository and GitHub state

| Item | State |
|---|---|
| Branch | `main` |
| Local commit | `df5ff2a` — merge #201 |
| Tracked remote ref | `origin/main` at the same commit |
| Before synchronization | Local was 7 commits behind tracked `origin/main` |
| Synchronization | Fast-forwarded to `origin/main` without changing application files manually |
| Fresh network fetch | Blocked by SSH host-key verification in this environment |
| Working tree | One untracked `STATUS.md`; no tracked changes |

The current comparison is against the available `origin/main` tracking
snapshot. A newer GitHub commit cannot be ruled out until a fresh authenticated
fetch succeeds.

## Real, local-only, and simulated components

| Component | Current truth | Environment / evidence |
|---|---|---|
| `credit-gate` | Real Gate Program with SAS reads, holder governor, block-list, lot ledger, `validate`, `settle_check`, and Token ACL hooks | Live devnet; real SAS and Token ACL integration tests; `DEPLOYMENTS.md` and `ops/gate/` |
| SAS + Token ACL + NPCS | Real frozen-by-default Token-2022 mint; holder admission/thaw and typed rejection are real | Live devnet; NPCS is `6vt1…VKL4`, wired through Token ACL → `credit-gate` |
| `payment-rails` | Real USDC cash layer: escrow, bonds, refunds, DvP cash, ROFR, advances, sweeps, coupon cash, crank and evidence | Live devnet and local/LiteSVM suites; recorded R5 used a manifest mint, not live NPCS |
| `credit-math` | Real fixed-point arithmetic, clearing, pro-rata, NAV, PV, waterfall, loss and prudential functions | Rust unit/property tests; consumed by the engine programs |
| `mark-engine` | Real program, feed, state machine and NAV publication path | Live devnet feed; NAV publication #1 = `1.000000`; no four-week history or auction print |
| `auction-engine` | Real sealed-bid, reveal, clear, pro-rata, ROFR, settlement and print code | Program/venue live on devnet; full scenarios proven locally/LiteSVM; no live session |
| `queue-claim` | Real redemption-claim mint, sale and pledge paths | Queue initialized on devnet; no live request, claim, sale or pledge |
| `liquidity-facility` | Real advance, health, stale-mark guard and liquidation ladder code | Local/LiteSVM only; not deployed on devnet |
| `npcs-treasury` | Real mint-on-deposit/redeem program | Binary deployed on devnet; treasury PDA and mint-authority handover are not active |
| `tranche-engine` | Real senior/junior, three-book and waterfall code | Binary deployed on devnet; no pool or coupon period exists |
| App | Real read views and wallet transaction builders; full session path works locally | Devnet gate/portfolio reads; session flow requires a live engine session; `/gate` intentionally simulates `validate` without sending |
| Services | Real indexer, API, NAV publisher, treasury agent, crank runner and exporter | Local validator/PGlite/devnet-readable; not hosted on Supabase/Helius/VPS |
| Crank | Real phase, refund, seasoning, settlement, distribution and evidence tooling | `payment-rails/crank`: 53/53 tests; live auction still blocked by state/deployment prerequisites |

There is no evidence that the core on-chain business logic is mocked. Local and
LiteSVM fixtures use synthetic accounts, clocks, balances, and attestations;
they are real program executions but are not live-devnet evidence.

## Program status

| Program / crate | Code | Devnet | Current meaningful proof | Main remaining work |
|---|---|---|---|---|
| `payment-rails` | Complete core | Live | Cash-layer R5, refunds, DvP cash, crank and evidence | Live NPCS R5; execute upgrade #3 and verify current build |
| `credit-gate` | CG-P1..P7 | Live | Real SAS/Token ACL admission and rejection | Custody hardening; broader live session usage |
| `credit-math` | Core tested | Library | Unit/property suites and engine consumers | Exercise remaining functions in production flows |
| `auction-engine` | AE-P1..P11 | Program + venue live | 3-bidder LiteSVM/local flow; cloned-devnet rehearsal | Rails grant, current-source redeploy, live NPCS session |
| `mark-engine` | ME-P1..P7 | Feed live | NAV #1 and local state/print tests | Four weekly marks, live auction print, hosted publisher |
| `queue-claim` | QC-P1..P5 | Queue live | Local sale and pledge paths | Live request, claim issuance and auction sale |
| `liquidity-facility` | LF-P1..P6 | **Not deployed** | Full local ladder and stale-mark protection | Deploy and grant rails facility role |
| `tranche-engine` | TE-P1..P7 | Binary live; no pool | Local pool, loss and coupon tests | Create pool, grant coupon role, execute a period |
| `npcs-treasury` | I1 logic | Binary live; no treasury state | Local Squads handover and backing tests | Propose/execute mint-authority handover and initialize PDA |

## SRS 08 acceptance scenarios

The strict SRS target is mainnet-beta, real USDC, reproducible evidence, and
all twelve scenarios. Every row below is therefore still partial at the
strict-pilot level, even where a devnet or local slice is real.

| AC | Scenario | Grade | Evidence and gap |
|---|---|---|---|
| AC-1 | Gate activation | **Partial** | Real devnet SAS/Token ACL/`credit-gate`; no mainnet pilot or hardened custody |
| AC-2 | Four weekly NAV marks and state transitions | **Partial** | Devnet feed and NAV #1; no four-week record, yield venue or mainnet history |
| AC-3 | Full three-bidder auction and DvP | **Partial** | Real local/LiteSVM execution; no recorded live-devnet session |
| AC-4 | Discount print and curve update | **Partial** | Local `0.95` vs `1.00` scenario; no live auction print |
| AC-5 | Partial ROFR and decline | **Partial** | Real local instruction/CPI path; no live issuer action |
| AC-6 | Lot seasoning and metadata-preserving split | **Partial** | Real gate ledger and local tests; no completed live auction path |
| AC-7 | Holder-cap rejection/reallocation | **Partial** | Real governor logic; no live auction reallocation evidence |
| AC-8 | Queue-claim sale and facility pledge | **Partial** | Both paths work in local/LiteSVM; no live claim or facility deployment |
| AC-9 | Maintenance breach and liquidation ladder | **Partial** | Local `ADMIN_DRILL` and full ladder; facility absent from devnet |
| AC-10 | Tranche pool and coupon waterfall | **Partial** | Local pool/waterfall tests; no live pool, coupon, or treasury yield |
| AC-11 | Crank timeout/refund | **Partial** | Real rails refund and validator drill; strict SRS mainnet evidence absent |
| AC-12 | Evidence export and mark reproduction | **Partial** | Local signed export works; no complete live pilot bundle or mark history |

## Hackathon / P0 status

### Substantive P0 table

| P0 item | Status | Current truth |
|---|---|---|
| NPCS + Token ACL + `credit-gate` + SAS | **Complete on devnet** | Holder H can thaw through the real gate; a stranger receives a typed rejection |
| `auction-engine` + `credit-math` | **Partial** | Deployed and tested; devnet venue has no session; cloned rehearsal only |
| Queue-claim sold in an auction | **Partial** | Local/LiteSVM sale works; no live request/claim/sale |
| Live NPCS DvP | **Not met** | Existing recorded R5 used a manifest mint; live-NPCS script is rehearsed only |
| Minimal mark engine | **Partial / live slice** | Feed and NAV #1 are live; four weekly marks and live print are missing |
| Liquidity facility | **Local-only** | Code and tests exist; no devnet deployment |
| Tranche waterfall | **Program-only / roadmap demo** | Binary is deployed; no pool or coupon period |
| UI | **Partial** | Devnet gate/portfolio views; local complete session; allowlist/admin/data-room exist |

The project should present the headline auction and queue rows as pending,
rather than as completed merely because the programs are deployed.

### Submission readiness

The following remain unchecked or unverified in `HACKATHON.md`:

- Colosseum team registration and India country selection
- Portal submission
- Pitch video and demo video
- Superteam India submission
- CertiK submission
- Adevar submission, tweet and follow
- Optional RPC Fast/Solami work

## Deviations from the architecture and SRS plan

1. The target is a real mainnet pilot; the current project is devnet/local.
2. The target NPCS is treasury-backed and mint-on-deposit; current backing is
   manually maintained until the `npcs-treasury` Squads handover executes.
3. The target headline proof includes a live auction and queue-claim sale;
   current proof is local/LiteSVM plus a cloned-devnet rehearsal.
4. The target queue attestation is transfer-agent/SAS-shaped; the current queue
   path uses a configured issuer/attester flow and is not a complete live TA
   integration.
5. The target authority model moves engine authorities to governance; engine
   programs currently remain under the deploy key.
6. Commit #201 added `ProgramData`/upgrade-authority checks to initialization
   and open instructions. At least the devnet mark, auction, and queue binaries
   predate those guards and must be rebuilt/redeployed/reverified.
7. The target UI calls for a polished Tailwind/shadcn/Tremor-style interface;
   the current UI uses plain CSS.
8. The target off-chain services include hosted Postgres, Helius webhooks and
   continuously operated jobs; the current services are executable but local or
   manually run.
9. `npcs-treasury` is an eighth Anchor program added to the build, while parts
   of the architecture inventory still describe only the original seven.

## Verification results on this snapshot

| Check | Result |
|---|---|
| App lint | Pass |
| App typecheck | Pass |
| App build | Pass |
| App tests | **32/37 pass**; 5 engine IDL drift failures |
| Services typecheck | Pass |
| Services tests | **18/19 pass**; decoder-copy synchronization failure |
| Crank tests | **53/53 pass** |
| Crank typecheck | Pass |
| Rust tests | Not run; `cargo` unavailable in the audit environment |
| Token tests | Not run; `tsx` unavailable in the token workspace |

The current checkout is not fully green. The app failures are caused by
generated `target/idl` files not matching committed app/crank IDLs after #201.
The services failure is caused by `services/src/codec.ts` not matching the
shared app decoder implementation.

## Documentation drift to resolve

The following documents contain stale or contradictory claims relative to the
current deployment registry and source:

- `HACKATHON.md`: old NPCS treasury and Token ACL status in sections 1 and 3.
- `README.md`: repository structure still describes services as not built in
  one location, despite the services implementation and status table.
- `services/README.md`: old statements that only `credit-gate` and
  `payment-rails` are deployed and that the mark feed is not on devnet.
- `token/TOKENOMICS.md` and `token/ARCHITECTURE.md`: pre-treasury and
  pre-Token-ACL-wiring descriptions.
- `payment-rails/PHASES.md`: superseded mint/gate references.
- SRS 08 historical as-built sections: several rows predate #171, #192 and
  #200 deployments and should be explicitly marked historical.
- `DEPLOYMENTS.md`: engine binary/source synchronization needs updating after
  current-source redeployment.

## Remaining work, ordered by launch impact

### P0: make the headline demo real

1. Rebuild the current #201 source with the pinned Anchor/Rust toolchain.
2. Regenerate and synchronize all target/app/crank IDLs.
3. Fix the services decoder synchronization failure.
4. Redeploy and reverify affected engine binaries; update `DEPLOYMENTS.md`.
5. Execute payment-rails upgrade #3 and create the auction engine grant.
6. Run the live NPCS DvP and sealed-bid auction on devnet.
7. Create and sell a real queue claim on the live venue.
8. Preserve transaction signatures, reconciliation output, transfer proof and
   signed evidence manifests.

### P1: make backing and operations real

1. Propose and execute the NPCS treasury mint-authority handover.
2. Initialize the treasury PDA and verify on-chain I1 enforcement.
3. Publish four weekly NAV marks and resolve the on-chain SAS-attestation gap
   in `mark-engine::publish_nav`.
4. Host the indexer/API/NAV/crank services if the demo depends on persistence.
5. Move engine authorities to the documented governance setup.

### P2: complete the broader SRS

1. Deploy and initialize `liquidity-facility`.
2. Create a tranche pool, grant the coupon role and execute a live period.
3. Complete all twelve scenarios on mainnet-beta with real USDC.
4. Engage counsel and schedule the external audit before mainnet launch.
5. Complete the human-owned hackathon and track submissions.

## Source references

- Architecture: `preresearch/architecture/06-Solution-Architecture-v2.md`
- SRS and acceptance scenarios: `preresearch/architecture/08-SRS-Pilot-Rails.md`
- Hackathon scope: `HACKATHON.md`
- Current implementation summary: `README.md`
- Deployment evidence: `DEPLOYMENTS.md`
- Services status: `services/README.md`
- UI status: `client/README.md`
