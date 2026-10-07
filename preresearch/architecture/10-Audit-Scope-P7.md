# 10 · External audit scope (P7) — request-for-quote pack

**Status (2026-10-07, #150): scope written, firms not yet contacted.** This
is the document SRS 08 §11 says to send for quotes. Booking the slot, signing
the engagement and paying for it are the founders' actions; nothing here
says an audit is booked. Update the **Booking** table below as each step
happens.

## Booking

| Step | Owner | Date | State |
|---|---|---|---|
| Send this pack to 2–3 Solana auditors (OtterSec / Neodyme / Zellic tier, or independents; CertiK and Adevar via the hackathon sidetracks, HACKATHON.md §3B) | founders | — | ❌ not sent |
| Quotes back; pick one; freeze scope at contract (SRS 08 §11: "scope growth is the #1 timeline risk") | founders | — | ❌ |
| Code freeze commit named in the contract (see **Freeze** below) | eng | — | ❌ |
| Audit window (assume 10 weeks elapsed, SRS 08 §11) | auditor | — | ❌ |
| Criticals/highs fixed and re-reviewed; mediums triaged in writing; report published | eng + auditor | — | ❌ |

## Scope

Line counts are non-blank, non-comment Rust lines in each program's `src/`
at `main` `14ae1d1` (2026-10-06). Recount at the freeze commit.

| Program | Path | Lines | Priority (SRS 08 §11) | Why it matters |
|---|---|---|---|---|
| `payment-rails` | `payment-rails/programs/payment-rails` | 4,368 | **Fixed scope** | Holds all client cash: escrow and bonds, never-pausable refunds, the DvP cash leg, ROFR, facility funding / advances / sweep, the coupon waterfall, bonded keepers, engine grants |
| `credit-gate` | `programs/credit-gate` | 2,757 | **Fixed scope** | sRFC37 gate: SAS attestation read, holder governor, block-list, lot ledger (seasoning), `validate`, the Token ACL thaw/freeze hooks and the settle-time `settle_check` every fill CPIs |
| `auction-engine` | `programs/auction-engine` | 1,286 | **Fixed scope** (escrow / clearing / settlement paths) | Sealed commitments, uniform-price clearing, pro-rata allocation, ROFR window, settlement through the rails, the print |
| `liquidity-facility` | `programs/liquidity-facility` | 1,208 | **Fixed scope** | Advances at the protective mark, H4 guard, the liquidation ladder (ROFR → backstop → flagged forced sale) |
| `credit-math` | `programs/credit-math` | 845 | **Fixed scope** | Every money path's arithmetic: NAV, clearing, pro-rata, waterfall, PV, accrual, prudential floors. Rounding direction is a protocol rule |
| `mark-engine` | `programs/mark-engine` | 726 | Best effort | NAV publication, mark states and haircuts, print ingestion |
| `queue-claim` | `programs/queue-claim` | 931 | Best effort | Redemption-queue claims: sell, pledge, sweep |
| `tranche-engine` | `programs/tranche-engine` | 914 | Best effort | Senior/junior pools, three-book accounting, waterfall through the rails |
| `npcs-treasury` | `programs/npcs-treasury` | 344 | Best effort (add if the pilot mints through it) | I1 mint-on-deposit treasury: the only NPCS mint path |
| **Total** | | **13,379** | fixed 10,464 · best effort 2,915 | |

Out of scope: the off-chain services and cranks (`services/`,
`payment-rails/crank`), the UI (`app/`), and third-party programs (Token-2022,
Token ACL, SAS, Squads), which are assumed correct. Integration *with*
them is in scope: account validation of every CPI target and every account
they hand back.

## What the auditor gets with the code (SRS 08 §11 "Prep")

| Item | Where |
|---|---|
| Requirement IDs per program (CG-P1..7, AE-P1..11, ME, QC, LF, TE) and the as-built grade | SRS 08 §4, §1.1 |
| Threat inventory the design answers (141 problems, ranked) | `05-Problem-Register.md` |
| Compliance requirements the programs encode | `07-Legal-Topology-and-Compliance.md` Part III |
| Asset invariants I1 (backing), I2 (NAV), I3 (eligibility) with exact integer math | `09-NPCS-Asset-Spec.md`, `token/TOKENOMICS.md` |
| LiteSVM integration suites against the real `.so` files and the real SAS / Token ACL programs (139 tests) | `payment-rails/tests-svm/`, `programs/credit-gate/tests-svm/` |
| Property tests + 100% line/branch coverage gate for `credit-math` | `programs/credit-math/tests/props.rs`, CI `credit-math — coverage` |
| libFuzzer targets: settle_fill, clearing, pro-rata, waterfall, refund-bond, facility | `payment-rails/fuzz/fuzz_targets/` |
| Kill-the-crank drill on a real validator (refund liveness, AC-11) | `payment-rails/crank` `npm run drill` |
| Deployed builds, hashes, upgrade authorities, Squads 2-of-2 + 48 h timelock | `DEPLOYMENTS.md`, `ops/squads/` |
| Known open items the auditor should not re-report as findings | **Known issues** below |

## Questions we want answered (beyond a line-by-line review)

1. **Cash conservation.** Can any instruction sequence across `payment-rails` and the engines move more USDC out of a vault than went in, or strand a user's funds? Refunds are designed to be permissionless and never pausable (AE-P10).
2. **Gate bypass.** Can NPCS reach a wallet `credit-gate` would not admit (thaw, settle, CPI from an engine, permanent-delegate paths), or can an unseasoned lot be sold?
3. **Engine authority.** `EngineGrant` roles let engines drive the rails. Can a non-granted program, or a granted one acting outside its role, open sessions, settle fills or distribute coupons?
4. **Rounding.** Every `credit-math` floor and ceiling has a stated beneficiary. Is any of them exploitable by repetition (dust extraction)?
5. **Liquidation under a stale mark.** AC-9 says no liquidation step runs on a Stale or Suspended mark. Confirm there is no path around it.
6. **Upgrade and authority model.** Which keys can change what, and is the Squads timelock really the only upgrade path? See DEPLOYMENTS.md §#93.

## Known issues (declared, not findings)

- Devnet runs older builds than `main`. payment-rails is at v0.1.0 plus a pending Squads upgrade (#172), and the engines are not deployed there. The audit is of the freeze commit, not of devnet.
- The Token ACL MintConfig authority for the devnet NPCS mint is still a single key (`7Udt…`, kept to thaw rails escrow vaults); see `token/README.md`.
- `publish_nav` checks the publisher set, not a SAS attestation of the NAV (SRS 08 §3.2 step 5 has no on-chain verifier).
- Prudential values H2/H3 and `MAX_HOLDER_CAP` are awaiting sign-off (`programs/credit-math/src/prudential.rs`).

## Freeze

The contract names one commit of this repository. From that commit:
program changes go only through the auditor's fix review; the `.so` hashes
of the audited build are recorded in `DEPLOYMENTS.md`; mainnet deploys only
that build (SRS 08 §11 / P8).
