# payment-rails — Interface contract

What the rails **expect** from the other NUNDINA components, and what they
**offer** in return. The rails implement the **cash layer only**. The auction
state machine, the gate and the token leg live elsewhere. Items marked 🔜
are agreed but not built yet (see `PHASES.md`).

Status legend: ✅ exists on this branch · 🔜 planned in the named phase · ❓ open question (PHASES.md §4)

---

## 1 · Settlement asset (USDC)

| Expectation | Why | Status |
|---|---|---|
| One settlement mint per deployment, recorded in `RailsConfig.usdc_mint` | Every escrow, bond and payout is denominated in it | ✅ |
| **6 decimals** | `credit-math` works in micro-USDC; any other precision rescales prices silently | ✅ enforced at init |
| Owned by SPL Token **or** Token-2022 (`token_interface`) | Devnet/mainnet USDC is classic SPL Token; a Token-2022 test mint must also work | ✅ |
| No transfer fee / transfer hook on the settlement mint | Settlement must conserve every micro-USDC (R2 exit criterion) | ✅ `deposit` checks the vault received exactly `cash + bond`, else `ConservationViolated`. A hook mint fails the CPI |
| Local tests use a **local 6-dp test mint**, never the live devnet USDC or the friend's NPCS mint | PHASES.md §2 | ✅ `tests-svm` fabricates one under each token program |

## 2 · From `token/` (NPCS, Token-2022)

The rails never hold NPCS custody accounts or sign for them. The one exception
is the DvP token leg, which the rails either sequence or delegate (❓ below).

| Expectation | Source | Status |
|---|---|---|
| NPCS mint: 6 dp, `DefaultAccountState=Frozen`, `PermanentDelegate`, no transfer hook, no transfer fee, no Confidential Transfers | `token/params.toml` `[extensions]` | ✅ on devnet (`DEPLOYMENTS.md`) |
| A participant's NPCS account is **thawed by the gate** before any fill that credits it | CG-P1 via Token ACL | ❓ Q2: does the token branch expose thaw before ~2 Oct? |
| Before merge, rails tests run against a **local mint built from the same extension manifest** | PHASES.md §2 | 🔜 R2 |
| The permanent delegate is **never** used by the rails. It's an issuer recovery tool, not a settlement path | SRS §8 | ✅ (the rails have no code path that could) |

## 3 · From `credit-gate`

The rails don't read SAS attestations themselves. They rely on the gate's
decisions and on a payout registry that only admits attested wallets.

| Expectation | Used by | Status |
|---|---|---|
| `MarketConfig` PDA at `["config", mint]` (gate program `7yRE…Qa9c`), readable to snapshot policy at session open | R2 settle-time re-check | ✅ skeleton on devnet |
| Final re-check before each fill (attestation still valid, block-list miss, seasoning OK). Rails require this to have **passed in the same transaction** before releasing cash | AE-P8 | 🔜 gate P1 · rails R2 |
| A payout destination is admitted to the rails registry only with a gate-valid owner. The registry stores `{owner, token_account, attestation_ref_hash}`, with the hash only and no raw ids | R3 private payouts | 🔜 R3 |
| Block-list takes precedence: a blocklisted owner's registry entry is unusable immediately, and refunds to it go to the escrow of record (never to a new address) | CG-P5 | 🔜 R3 |

## 4 · From `auction-engine` (caller of the rails)

❓ Q1: who builds the auction state machine is still open. The rails expose
**cash primitives**. The engine owns phases, clearing and reveal checking.
Until it exists, the rails keep their own minimal `CashSession` holding the
deadlines, and the **operator** (`RailsConfig.operator`, re-pointable with
`set_operator`) stands in for the engine.

| The engine provides | The rails do | Phase |
|---|---|---|
| Its session-signer PDA, installed as `RailsConfig.operator` by the authority | Only that key can `open_session`, `mark_revealed` and `fail_session`. None of those move cash | ✅ R1 |
| At open: `session_id`, `commit_end < reveal_end < settle_deadline`, bond size, per-investor cap | `CashSession` at `["session", operator, id]` snapshots these plus the mint and reserve. They're immutable after open (L5) | ✅ R1 |
| Commit: the bidder's cash amount (`qty × limit`, computed off-chain so the bid stays sealed) | `deposit(cash)`: the bidder signs, `cash + bond` goes into a per-escrow vault. One escrow per `(session, owner)`. Rejected when paused or depegged | ✅ R1 |
| Reveal outcome: `mark_revealed` for each preimage that checked out, inside the reveal window | Unrevealed escrows become forfeitable at `reveal_end`: `forfeit_bond` (permissionless) or `refund` sends the bond to the reserve (AE-P4) | ✅ R1 |
| Abort (insufficient commits, no cross ≥ floor): `fail_session` | Every escrow is refundable at once. Bonds are forfeited only if reveals had already closed | ✅ R1 |
| Nothing, since the rails read their own deadlines | `refund` is **permissionless and never pausable**. It pays only the owner's token account and closes the vault and escrow (AE-P10, AC-11) | ✅ R1 |
| One fill: `(fill_index, buyer, seller, qty, clearing_price, rofr)` | `settle_fill` releases `qty × price` to the seller, splits fee and reserve, and refunds the excess `(limit − price) × qty` to the buyer. A fill-receipt PDA `["fill", session, fill_index]` makes a replay fail with `FillAlreadySettled` | 🔜 R2 |
| ROFR exercise at the clearing price | The issuer multisig pays cash through the same `settle_fill` path (`rofr = true`) | 🔜 R2 |
| `ClearingPrint` (engine event) | Nothing. The rails emit `FillSettled` per fill and never an aggregate print | — |

**Trust assumption (R1):** the operator reports reveals. An operator that
withholds `mark_revealed` can make an honest bidder's bond forfeit. It can
never move or redirect cash. When the engine is a program, the operator is
its PDA, and this assumption reduces to trusting the audited engine.

**Conservation invariant (R2):** for every fill,
`to_seller + fee + to_reserve + buyer_refund == escrow_released`, computed in
u128 through `credit-math` with rounding against the initiator. Otherwise the
fill fails with `ConservationViolated`.

## 5 · From `queue-claim` / `liquidity-facility`

| Expectation | Rails behaviour | Phase |
|---|---|---|
| Claim sale proceeds identify an optional pledge `(facility, advance)` | Sweep order: **facility repayment first, then the claim holder** (QC-P4), as `PayoutMade{FacilityRepayment}` then `PayoutMade{ClaimProceeds}` | 🔜 R3 |
| Facility fund PDA (seeded from the team treasury in the pilot) | `FacilityAdvance` payout only to a registered destination | 🔜 R3 |
| Interest parameters (pilot: fixed 12% APR) | Accrued with `credit_math::accrue_interest`, never local math | 🔜 R3 |
| Tranche period cash plus `WaterfallParams` | `credit_math::distribute_cash`, where the lines must sum exactly to cash in | 🔜 R3 |

## 6 · What the rails offer everyone

- **Config:** `RailsConfig` at `["rails_config"]`. Read `paused` / `depeg_breaker` / `operator`. Sessions snapshot `usdc_mint` and `reserve` at open. ✅
- **Errors:** `RailsError` codes are append-only and wire-stable (6000+). `credit_math::MathError` maps 1:1 onto the `Math*` variants. ✅
- **Events:** `RailsConfigUpdated`, `BreakerChanged`, `SessionEvent` and `EscrowEvent` are emitted as of R1. `FillSettled` (R2) and `PayoutMade` (R3) are defined but not emitted yet. Each carries wallets and amounts plus 32-byte hashes, with no PII. Every cash movement emits exactly one event, and `CashSession.total_cash` / `total_bonds` track live escrows, so the R4 ledger can reconcile `sum(open escrows) == vault balances`. ✅
- **Liveness guarantee:** a user can always get unfilled cash back through a permissionless refund, even when the rails are paused, the breaker is tripped, or the crank is dead. ✅ R1 (LiteSVM: `refunds_work_while_paused_and_depegged`, `third_party_refund_cannot_redirect_cash`)

## 7 · Authorities (SRS §8)

| Role | Key | Can | Cannot |
|---|---|---|---|
| Rails authority | Program upgrade authority at init (Squads 2-of-2 from P1) | Init config; set the operator, pause and depeg breaker (treasury re-pointing: R2) | Move escrowed user cash; block refunds |
| Operator | Auction-engine signer (the authority until Q1 lands) | Open sessions, report reveals, fail a session | Move or redirect cash |
| Crank | Any keypair | Settle / refund once conditions hold | Anything discretionary |
| Participants | Wallets | Deposit, refund their own or anyone's escrow after the deadline | Admin functions |
