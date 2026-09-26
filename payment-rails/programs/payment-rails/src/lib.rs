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
//! typed errors, event families); R1 — cash sessions and per-escrow vaults:
//! deposit (cash + forfeitable bond), operator-reported reveals, bond
//! forfeit to the reserve, and a permissionless refund that no pause,
//! breaker or dead crank can block. The DvP cash leg (R2) and the payout
//! router (R3) land on this frame. No `unsafe`; the only CPIs are SPL
//! Token / Token-2022 `transfer_checked` and `close_account`.

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

    /// AE-P1/P3: open a cash session with snapshotted deadlines (operator).
    pub fn open_session(
        ctx: Context<OpenSession>,
        session_id: u64,
        commit_end: i64,
        reveal_end: i64,
        settle_deadline: i64,
        bond_amount: u64,
        max_cash: u64,
    ) -> Result<()> {
        instructions::session::process_open_session(
            ctx,
            session_id,
            commit_end,
            reveal_end,
            settle_deadline,
            bond_amount,
            max_cash,
        )
    }

    /// AE-P3: lock cash + bond in a per-escrow vault (depositor signs).
    pub fn deposit(ctx: Context<Deposit>, cash: u64) -> Result<()> {
        instructions::escrow::process_deposit(ctx, cash)
    }

    /// AE-P4: operator reports a successful reveal.
    pub fn mark_revealed(ctx: Context<OperatorEscrow>) -> Result<()> {
        instructions::session::process_mark_revealed(ctx)
    }

    /// AE-P2/P4: operator aborts the session; all escrows refundable.
    pub fn fail_session(ctx: Context<FailSession>) -> Result<()> {
        instructions::session::process_fail_session(ctx)
    }

    /// AE-P4: unrevealed bond → reserve after reveals close (permissionless).
    pub fn forfeit_bond(ctx: Context<ForfeitBond>) -> Result<()> {
        instructions::escrow::process_forfeit_bond(ctx)
    }

    /// AE-P10 / AC-11: permissionless refund; never pausable.
    pub fn refund(ctx: Context<Refund>) -> Result<()> {
        instructions::escrow::process_refund(ctx)
    }
}
