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
//! R0 scope (this skeleton): the `RailsConfig` anchor — one governed,
//! evented, upgrade-authority-initialized PDA — plus the typed error set and
//! the event families every later instruction must emit. Escrow/bond vaults
//! (R1), the DvP cash leg (R2) and the payout router (R3) land on this
//! frame. This crate carries no `unsafe` and no CPIs yet.

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
}
