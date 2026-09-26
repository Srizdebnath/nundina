//! `payment-rails` — the USDC money layer for NUNDINA.
//!
//! SRS 08 §4.2 (AE-P3/P4/P7/P8/P10), §4.4 (QC-P4), §4.5 (LF-P1/P2/P6) ·
//! payment-rails/PHASES.md. Audit-scope-critical: every micro-USDC that
//! moves in the protocol moves through this program. The security itself
//! never leaves Solana and never passes through here — only cash does.
//!
//! "Private" in this program means (PHASES.md §1): payouts go only to
//! attested, allowlisted destinations, and nothing PII-bearing is written
//! on-chain — off-chain references appear only as 32-byte hashes. There are
//! no Confidential Transfers (incompatible with the permissioned book,
//! 05 D2 / 09 §2).
//!
//! Built so far: R0 — the governed `RailsConfig` (upgrade-authority init,
//! typed errors, event families); R1 — cash sessions and per-escrow vaults
//! with permissionless, never-pausable refunds; R2 — seller asset escrow and
//! atomic DvP settlement (`settle_fill`, one fill per instruction,
//! idempotent via a fill-receipt PDA), the fee/reserve split through
//! `credit-math`, the issuer ROFR cash path and depeg-breaker halting; R3 —
//! the payout router: a registrar-maintained registry of attested payout
//! wallets (hash-only), facility advances/repayments with `credit-math`
//! interest, the QC-P4 sweep (facility first, then holder) and TE-P4 coupon
//! distribution through the `credit-math` waterfall. No `unsafe`; the only
//! CPIs are SPL Token / Token-2022 `transfer_checked` and `close_account`.

#![deny(unsafe_code)]
// missing_docs activates once the hand-written account surface dominates;
// anchor-lang 1.1 macro-generated items trigger it spuriously.
#![allow(missing_docs)]
// anchor-lang's generated code emits cfgs (custom-heap, anchor-debug, …)
// declared in ITS crate, so rustc flags them here as unexpected under
// `-D warnings`. They are expected (same allowance as credit-gate).
#![allow(unexpected_cfgs)]

pub mod error;
pub mod events;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use error::RailsError;
pub use events::*;
pub use instructions::*;
pub use state::*;

declare_id!("GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn");

/// Settlement asset decimals. `credit-math` operates in micro-USDC; a mint
/// with any other precision would silently rescale every price (D8).
pub const USDC_DECIMALS: u8 = 6;

#[program]
pub mod payment_rails {
    use super::*;

    /// Initialize the global rails config. Only the program's upgrade
    /// authority (Squads 2-of-2, SRS §8) may call this, so the singleton
    /// PDA cannot be front-run at deploy time.
    pub fn initialize_config(ctx: Context<InitializeConfig>) -> Result<()> {
        instructions::initialize_config::handler(ctx)
    }

    /// Re-point the session operator (authority only; evented).
    pub fn set_operator(ctx: Context<AdminConfig>, operator: Pubkey) -> Result<()> {
        instructions::admin::process_set_operator(ctx, operator)
    }

    /// Flip the pause or depeg breaker (authority only; evented). Never
    /// blocks refunds.
    pub fn set_breaker(
        ctx: Context<AdminConfig>,
        breaker: Breaker,
        engaged: bool,
        evidence_hash: [u8; 32],
    ) -> Result<()> {
        instructions::admin::process_set_breaker(ctx, breaker, engaged, evidence_hash)
    }

    /// Set fee / reserve bps within the hard ceilings (authority; evented).
    pub fn set_fees(ctx: Context<AdminConfig>, fee_bps: u16, reserve_bps: u16) -> Result<()> {
        instructions::admin::process_set_fees(ctx, fee_bps, reserve_bps)
    }

    /// R3: re-point the payout registrar (authority only; evented).
    pub fn set_registrar(ctx: Context<AdminConfig>, registrar: Pubkey) -> Result<()> {
        instructions::admin::process_set_registrar(ctx, registrar)
    }

    /// AE-P1/P3: open a cash session with snapshotted parameters (operator).
    pub fn open_session(
        ctx: Context<OpenSession>,
        session_id: u64,
        params: SessionParams,
    ) -> Result<()> {
        instructions::session::process_open_session(ctx, session_id, params)
    }

    /// AE-P3: lock cash + bond in a per-escrow vault (depositor signs).
    pub fn deposit(ctx: Context<Deposit>, cash: u64) -> Result<()> {
        instructions::escrow::process_deposit(ctx, cash)
    }

    /// Sell side, step 1: create the seller's empty asset vault so a
    /// frozen-by-default asset (NPCS) can be thawed before `deposit_ask`.
    pub fn open_asset_vault(ctx: Context<OpenAssetVault>) -> Result<()> {
        instructions::escrow::process_open_asset_vault(ctx)
    }

    /// AE-P3 sell side: lock asset + bond in per-escrow vaults (seller signs).
    pub fn deposit_ask(ctx: Context<DepositAsk>, qty: u64) -> Result<()> {
        instructions::escrow::process_deposit_ask(ctx, qty)
    }

    /// AE-P4: operator reports a successful reveal.
    pub fn mark_revealed(ctx: Context<OperatorEscrow>) -> Result<()> {
        instructions::session::process_mark_revealed(ctx)
    }

    /// AE-P2/P4: operator aborts the session; all escrows refundable.
    pub fn fail_session(ctx: Context<OperatorSession>) -> Result<()> {
        instructions::session::process_fail_session(ctx)
    }

    /// R2: operator closes settlement; remaining escrow refundable at once.
    pub fn finish_session(ctx: Context<OperatorSession>) -> Result<()> {
        instructions::session::process_finish_session(ctx)
    }

    /// AE-P8: one atomic DvP fill between escrows (operator; idempotent).
    pub fn settle_fill(
        ctx: Context<SettleFill>,
        fill_index: u32,
        qty: u64,
        price: u64,
        disclosure_hash: [u8; 32],
    ) -> Result<()> {
        instructions::settle::process_settle_fill(ctx, fill_index, qty, price, disclosure_hash)
    }

    /// AE-P7: issuer ROFR fill — issuer pays cash, takes the seller's asset.
    pub fn settle_rofr_fill(
        ctx: Context<SettleRofrFill>,
        fill_index: u32,
        qty: u64,
        price: u64,
        disclosure_hash: [u8; 32],
    ) -> Result<()> {
        instructions::settle::process_settle_rofr_fill(ctx, fill_index, qty, price, disclosure_hash)
    }

    /// AE-P4: unrevealed bond → reserve after reveals close (permissionless).
    pub fn forfeit_bond(ctx: Context<ForfeitBond>) -> Result<()> {
        instructions::escrow::process_forfeit_bond(ctx)
    }

    /// AE-P10 / AC-11: permissionless buy-side refund; never pausable.
    pub fn refund(ctx: Context<Refund>) -> Result<()> {
        instructions::escrow::process_refund(ctx)
    }

    /// AE-P10: permissionless sell-side refund (asset + bond); never pausable.
    pub fn refund_ask(ctx: Context<RefundAsk>) -> Result<()> {
        instructions::escrow::process_refund_ask(ctx)
    }

    // ── R3 · payout router ──────────────────────────────────────────────

    /// Admit an attested payout wallet (registrar; hash only, no PII).
    pub fn register_destination(
        ctx: Context<RegisterDestination>,
        attestation_ref_hash: [u8; 32],
        expires_at: i64,
    ) -> Result<()> {
        instructions::payout::process_register_destination(ctx, attestation_ref_hash, expires_at)
    }

    /// Refresh / re-point / re-activate a payout wallet (registrar).
    pub fn renew_destination(
        ctx: Context<RenewDestination>,
        attestation_ref_hash: [u8; 32],
        expires_at: i64,
    ) -> Result<()> {
        instructions::payout::process_renew_destination(ctx, attestation_ref_hash, expires_at)
    }

    /// Revoke a payout wallet immediately (registrar).
    pub fn revoke_destination(ctx: Context<RevokeDestination>) -> Result<()> {
        instructions::payout::process_revoke_destination(ctx)
    }

    /// LF-P2: open a facility fund + vault (operator becomes curator).
    pub fn open_facility(
        ctx: Context<OpenFacility>,
        facility_id: u64,
        rate_bps: u64,
    ) -> Result<()> {
        instructions::facility::process_open_facility(ctx, facility_id, rate_bps)
    }

    /// LF-P2: add cash to a facility (anyone).
    pub fn fund_facility(ctx: Context<FundFacility>, amount: u64) -> Result<()> {
        instructions::facility::process_fund_facility(ctx, amount)
    }

    /// LF-P1: advance to a registered destination (curator).
    pub fn advance_funds(
        ctx: Context<AdvanceFunds>,
        amount: u64,
        collateral_ref_hash: [u8; 32],
    ) -> Result<()> {
        instructions::facility::process_advance(ctx, amount, collateral_ref_hash)
    }

    /// LF-P6: repay an advance, interest first (anyone).
    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        instructions::facility::process_repay(ctx, amount)
    }

    /// QC-P4: claim-sale proceeds → facility first, remainder → holder.
    pub fn sweep_proceeds(ctx: Context<SweepProceeds>, amount: u64) -> Result<()> {
        instructions::facility::process_sweep_proceeds(ctx, amount)
    }

    /// TE-P4: one coupon period down the `credit-math` waterfall.
    pub fn distribute_coupon(
        ctx: Context<DistributeCoupon>,
        period_id: u64,
        cash: u64,
        params: CouponParams,
        period_ref_hash: [u8; 32],
    ) -> Result<()> {
        instructions::coupon::process_distribute_coupon(
            ctx,
            period_id,
            cash,
            params,
            period_ref_hash,
        )
    }
}
