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

Since R2, the rails **do** hold the traded asset, but only inside a seller's
own ask escrow (`["asset_vault", escrow]`, authority = the escrow PDA), and
only between commit and settlement or refund. `settle_fill` moves it to the
buyer in the same instruction as the cash (true DvP).

| Expectation | Source | Status |
|---|---|---|
| NPCS mint: 6 dp, `DefaultAccountState=Frozen`, `PermanentDelegate`, no transfer hook, no transfer fee, no Confidential Transfers | `token/params.toml` `[extensions]` | ✅ on devnet (`DEPLOYMENTS.md`) |
| A buyer's NPCS account is **thawed by the gate** before any fill that credits it (otherwise `settle_fill` fails and nothing moves) | CG-P1 via Token ACL | ❓ Q2: does the token branch expose thaw before ~2 Oct? |
| **The rails' ask-escrow asset vaults must be thawed too.** With `DefaultAccountState=Frozen` a new vault starts frozen. Since R5 the sell side is two steps: the seller calls `open_asset_vault` (creates the empty vault `["asset_vault", escrow]`, owner = the seller's escrow PDA of program `GdWm…Pocn`), **the gate thaws it**, then `deposit_ask` fills it (a still-frozen vault fails with typed `AssetVaultFrozen`, 6041). The gate has to recognise rails vault PDAs as eligible escrow holders and **never** count them toward the holder cap (they aren't investors). Tested against a real NPCS-manifest mint in `tests-svm/tests/npcs.rs` | CG-P1/CG-P4 | ❓ gate side still to build; rails side ✅ R5 |
| Before merge, rails tests run against a **local mint built from the same extension manifest** | PHASES.md §2 | 🔜 R5. The R2 LiteSVM tests use a plain Token-2022 6-dp asset mint, so frozen-default is **not** exercised yet |
| The permanent delegate is **never** used by the rails. It's an issuer recovery tool, not a settlement path | SRS §8 | ✅ (the rails have no code path that could) |

## 3 · From `credit-gate`

The rails don't read SAS attestations themselves. They rely on the gate's
decisions and on a payout registry that only admits attested wallets.

| Expectation | Used by | Status |
|---|---|---|
| `MarketConfig` PDA at `["config", mint]` (gate program `7yRE…Qa9c`), readable to snapshot policy at session open | R2 settle-time re-check | ✅ skeleton on devnet |
| Final re-check before each fill (attestation still valid, block-list miss, seasoning OK). Rails should require this to have **passed in the same transaction** before releasing cash | AE-P8 | 🔜 gate P1. **`settle_fill` does not verify it yet**; it lands once the gate exposes the check (Q2) |
| A payout destination is admitted to the rails registry only with a gate-valid owner. The registry stores `{owner, token_account, attestation_ref_hash, expires_at, active}`, with the hash only and no raw ids | R3 private payouts | ✅ R3. Today the **registrar** key (`set_registrar`) attests. Once the gate's SAS read is on-chain (Q2), registration should also verify it |
| Block-list takes precedence: a blocklisted owner's registry entry is unusable immediately (`revoke_destination` → `DestinationRevoked` on every R3 payout path), and escrow refunds still go only to the escrow owner of record, never to a new address | CG-P5 | ✅ R3. The registrar must mirror gate block-list changes; there's no automatic sync yet |

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
| Sellers' asks: the seller escrows `qty` asset | `open_asset_vault` then `deposit_ask(qty)`: asset plus a USDC bond into the seller's own vaults (one transaction for an unfrozen asset; gate thaw in between for NPCS). Same one-escrow-per-owner rule, so a wallet can't be both buyer and seller in one session | ✅ R2 |
| One fill: `(fill_index, buyer, seller, qty, price)` | `settle_fill`, atomic DvP. The buyer's escrow pays `gross = ceil(qty × price / 10^dec)`, split into seller / fee / reserve through `credit-math`, and `qty` asset goes seller → buyer. Only revealed escrows on opposite sides, within their escrowed amounts, inside `[reveal_end, settle_deadline)`, and not while paused or depegged. A fill-receipt PDA `["fill", session, fill_index]` is created per fill, so a **replayed index fails** (the account already exists) and pays nothing | ✅ R2 |
| Settlement done | `finish_session` (operator, after `reveal_end`). The session becomes `Settled`, and buyer excess, unsold asset and bonds are refundable at once, through the same permissionless `refund` / `refund_ask` | ✅ R2 |
| ROFR exercise at the clearing price | `settle_rofr_fill`. The session's `rofr_authority` (set at open, or disabled) signs and pays cash from its own account, and takes the seller's escrowed asset into its own asset account | ✅ R2 |
| `ClearingPrint` (engine event) | Nothing. The rails emit `FillSettled` per fill and never an aggregate print | — |

**Trust assumption:** the operator reports reveals and chooses fills. An
operator that withholds `mark_revealed` can make an honest bidder's bond
forfeit, and it could apply a wrong clearing (a different matching or price)
within escrowed amounts. It can **never** move value to anyone outside the
fill's parties and the fee and reserve accounts recorded at session open. When the engine is a program, the operator is
its PDA, and this assumption reduces to trusting the audited engine.

**Conservation invariant (R2, ✅):** for every fill,
`to_seller + fee + to_reserve == gross`, computed in u128 through
`credit-math`. The buyer pays the ceiling, and fee and reserve are floored,
so nothing is created or lost. The fill receipt records the split. LiteSVM
checks that total USDC **and** total asset are unchanged after every fill and
every refund. `FillAlreadySettled` stays reserved in the error set; a replay
is actually rejected by the receipt account already existing.

## 5 · From `queue-claim` / `liquidity-facility` / `tranche-engine`

| Expectation | Rails behaviour | Phase |
|---|---|---|
| The facility engine signs as the **curator** (the config operator at `open_facility`). Underwriting (advance rate vs `MARK_PROTECTIVE`, the 50% hard ceiling, health checks, the liquidation ladder) stays in that engine | `open_facility(id, rate_bps)` creates a fund and its vault. `rate_bps` is the **interest APR**, capped by the rails' `MAX_RATE_BPS` (5 000 = 50% APR). That cap is not the doc 06 §11 H1 advance-rate ceiling, which is `credit_math::MAX_ADVANCE_RATE_BPS` and belongs to the facility engine. `fund_facility` is open to anyone. `advance_funds(amount, collateral_ref_hash)` pays **only** the borrower's registered destination, one live advance per `(facility, borrower)`, blocked while paused or depegged | ✅ R3 |
| Interest parameters (pilot: fixed 12% APR) | Accrued with `credit_math::accrue_interest` (simple, floored). `repay` applies interest first, never collects more than is owed, and closes the advance at zero | ✅ R3 |
| Queue-claim: the proceeds of a **pledged** claim's sale, signed for by whoever holds them | `sweep_proceeds(amount)`: **facility repayment first**, then the remainder to the holder's registered destination (QC-P4), emitted as `PayoutMade{FacilityRepayment}` then `PayoutMade{ClaimProceeds}`. The two legs sum exactly to `amount` | ✅ R3 |
| Proceeds of an **unpledged** claim | Nothing extra: `settle_fill` already pays the seller directly | ✅ R2 |
| Tranche-engine: period cash plus `CouponParams` (a Borsh mirror of `credit_math::WaterfallParams`, including its reserve ledger level), signed by the config operator | `distribute_coupon(period, cash, params, ref_hash)`: fees → fee receiver, reserve top-up → reserve, senior coupon + amortization → senior's registered destination, junior coupon + residual → junior's. **Fails unless the lines sum exactly to `cash`**. One receipt per `(distributor, period)` | ✅ R3 |

## 6 · What the rails offer everyone

- **Config:** `RailsConfig` at `["rails_config"]`. Read `paused` / `depeg_breaker` / `operator`. Sessions snapshot `usdc_mint` and `reserve` at open. ✅
- **Errors:** `RailsError` codes are append-only and wire-stable (6000+). `credit_math::MathError` maps 1:1 onto the `Math*` variants. ✅
- **Events:** all families are emitted as of R3: `RailsConfigUpdated`, `BreakerChanged`, `SessionEvent`, `EscrowEvent`, `FillSettled`, `PayoutMade` (advances, repayments, sweeps and coupon lines), `DestinationEvent` (registry writes, hash only) and `FacilityEvent`. Each carries wallets and amounts plus 32-byte hashes, with no PII. Every cash movement emits exactly one event, and `CashSession.total_cash` / `total_bonds` track live escrows, so the R4 ledger can reconcile `sum(open escrows) == vault balances`. ✅
- **Liveness guarantee:** a user can always get unfilled cash back through a permissionless refund, even when the rails are paused, the breaker is tripped, or the crank is dead. This includes a vault drained below its recorded amount by a token delegate: the refund returns what is held and emits `AssetShortfall`/`CashShortfall` (#62). Caveat: a vault **frozen** by the asset's freeze authority blocks `refund_ask` until it is thawed. ✅ R1 (LiteSVM: `refunds_work_while_paused_and_depegged`, `third_party_refund_cannot_redirect_cash`)
- **Crank, ledger and evidence (R4):** `crank/` settles fills and runs refunds idempotently, keyed on on-chain state: a receipt that exists, or an escrow that's gone. `reconcile` proves escrow balances equal open obligations, and `export` writes every cash event plus a SHA-256 manifest. The **kill-the-crank drill** runs on a real validator: the crank dies mid-plan and never finishes, and a participant's own wallet still refunds everyone. ✅ R4

## 7 · Authorities (SRS §8)

| Role | Key | Can | Cannot |
|---|---|---|---|
| Rails authority | Program upgrade authority at init (Squads 2-of-2 from P1) | Init config; set the operator, pause, depeg breaker, and fees (≤ hard caps) | Move escrowed user cash or asset; block refunds; exceed fee caps |
| Operator | Auction-engine signer (the authority until Q1 lands) | Open, fail and finish sessions; report reveals; settle fills between escrows | Move or redirect value outside a fill's parties and the recorded fee/reserve accounts |
| Registrar | Set by the authority (`set_registrar`); meant to be the SAS attester's key | Register, renew and revoke payout destinations | Move any cash |
| Curator | The facility engine's signer (the config operator at `open_facility`) | Advance to registered destinations; its facility's rent | Pay an unregistered wallet; exceed the rate cap |
| Issuer (ROFR) | `rofr_authority` recorded per session (optional) | Buy a seller's escrowed asset at the fill price with its own cash | Touch any buyer's escrow |
| Crank | Any keypair | Settle / refund once conditions hold | Anything discretionary |
| Participants | Wallets | Deposit, refund their own or anyone's escrow after the deadline | Admin functions |
