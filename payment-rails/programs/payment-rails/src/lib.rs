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
//! `credit-math`, the issuer ROFR cash path and depeg-breaker halting. The
//! payout router (R3) lands on this frame. No `unsafe`; the only CPIs are
//! SPL Token / Token-2022 `transfer_checked` and `close_account`.

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
}
