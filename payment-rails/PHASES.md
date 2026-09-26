# payment-rails — Context & Phase Plan

> Branch: `feat/private-rails` · Owner: payment rails · Token work lives on a separate branch (`token/`), merged to `main` independently.
> Hard deadline: **12 Oct 2026, 11:59 PM PT**. Target: rails working end to end on devnet by **~7 Oct** (HACKATHON.md: P0 E2E by ~day 12).

## 1 · Where the rails fit

NUNDINA never moves the security off Solana; **only cash moves** (HACKATHON.md §D). Every money path in the SRS is a USDC movement:

| Flow | SRS ref | What the rails do |
|---|---|---|
| Auction commit escrow + bond | AE-P3 | Lock `qty × limit` USDC + a forfeitable bond in a PDA |
| Unrevealed bid penalty | AE-P4 | Bond → protocol reserve |
| Per-fill DvP settlement | AE-P8 | Cash leg: escrow → seller, fees → fee/reserve, excess → buyer refund. Idempotent, one fill per ix |
| Timeout refunds | AE-P10 / AC-11 | `refund` permissionless, any participant, every phase |
| Issuer ROFR | AE-P7 | Issuer multisig pays cash at clearing price |
| Queue-claim sale proceeds | QC-P3/P4 | Sweep order: facility repayment → claim holder |
| Facility advance / repay | LF-P1/P2/P6 | USDC out of facility fund PDA, interest via `credit-math::accrue_interest` |
| Tranche coupons | TE-P4 | Cash waterfall via `credit-math::distribute_cash` |
| Depeg breaker | 06 §2, F4/M5 | Settlement-asset check → halt |

**"Private" means** that payments go only to **attested, allowlisted wallets**, with no PII on-chain (only reference hashes), and that detail is exposed as aggregate-only prints. Confidential Transfers are **out of scope** because they are incompatible with the permissioned book (05 D2, 09 §2). Privacy comes from commit–reveal plus disclosure policy, not from encrypted amounts.

## 2 · Boundaries (friend's warning)

- **Everything lives in `payment-rails/`.** Nothing outside this folder is edited.
- The folder has its **own Cargo/Anchor workspace**. `credit-math` is consumed **read-only** via a path dependency (`path = "../../../programs/credit-math"` from `programs/payment-rails/Cargo.toml`).
- Before merge, the token leg is tested against a **local test mint with the NPCS extension manifest** (`token/params.toml`), not the friend's live mint.
- Root changes needed **at merge time only** (listed here so they're agreed, not made): add the program to root `Cargo.toml` members and `Anchor.toml`, and add the rails to `.github/workflows/ci.yml`.

## 3 · Phases

| Phase | Days | Deliverable | Exit criteria |
|---|---|---|---|
| **R0 Scaffold** | 25–26 Sep | `payment-rails/` Anchor workspace, program skeleton, `credit-math` path-dep, README, interface doc (what rails expect from token/gate/auction) | `cargo test` + `anchor build` green inside the folder |
| **R1 Escrow vaults** | 26–29 Sep | USDC escrow + bond PDAs (`has_one`, no engine-owned balances), deposit, bond forfeit → reserve, **permissionless timeout refund** | LiteSVM tests: deposit/refund conservation, griefing cases, AC-11 cash side |
| **R2 DvP cash leg** | 29 Sep–2 Oct | `settle_fill` (one fill per ix, fill-receipt PDA for idempotency), seller payout, fee/reserve split, buyer excess refund, ROFR cash path, depeg breaker flag | Settlement conserves every micro-USDC; replayed fill rejected; AC-3 cash leg green locally |
| **R3 Payout router ("private")** | 2–4 Oct | Payout-destination registry (attested wallets only, hashed refs), sweep order (facility → holder), facility advance/repay, coupon distribution via `credit-math` | Non-allowlisted destination rejected with typed reason; waterfall sums exactly to cash in |
| **R4 Crank + ledger (TS)** | 4–6 Oct | Settlement/refund crank (idempotent, retry + priority fees), reconciliation (escrow balances == open obligations), evidence export for cash events | Kill-the-crank drill: refunds complete via any participant |
| **R5 Devnet + integration** ✅ | 6–8 Oct | Deploy to devnet, E2E with devnet USDC; after friend's merge, swap test mint → NPCS | Demo script runs: commit → settle → payout on devnet — **met** (DEPLOYMENTS.md). NPCS leg validated against a manifest mint; live NPCS swap waits on the gate thaw |
| **R6 Tempo payout (stretch)** | 8–11 Oct | USDC payout to a Tempo address (cash only; the security never leaves Solana) | Only if R5 is green **and** Colosseum confirms multi-track eligibility |
| **Freeze** | 11–12 Oct | Docs, demo recording support, merge PR | — |

## 4 · Open questions

1. **Auction ownership:** the rails implement the **cash layer** that `auction-engine` / `queue-claim` / `liquidity-facility` call into. Who builds the auction state machine (commit/reveal/clear)?
2. **Token leg of DvP:** the rails CPI into Token-2022 for the NPCS transfer, which needs the gate thaw to be working. Does the token branch expose that before ~2 Oct?
3. **Gate thaw for rails escrow vaults (from R2):** NPCS has `DefaultAccountState=Frozen`, so the rails' ask-escrow asset vaults (`["asset_vault", escrow]`, owned by an escrow PDA of the rails program) start frozen. The gate must thaw them as escrow holders **without** counting them toward the 12(g) holder cap. Otherwise `deposit_ask` can't receive NPCS. **R5:** confirmed against a real NPCS-manifest mint (the old one-shot `deposit_ask` failed with `AccountFrozen`); the rails now split out `open_asset_vault` so the gate can thaw in between (`tests-svm/tests/npcs.rs`). Gate side still open.
4. **Issuer clawback vs. refund liveness (R5 finding):** NPCS's PermanentDelegate can pull asset out of an ask vault. `refund_ask` then fails `ConservationViolated` forever, stranding the seller's USDC bond (contradicts AE-P10). Needs a policy call: refund what is held + the bond and record the shortfall, or keep failing loudly.
