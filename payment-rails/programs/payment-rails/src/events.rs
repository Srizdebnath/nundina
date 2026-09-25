//! Event families. Every rails state change emits exactly one event from
//! these families; the R4 ledger reconciles escrow balances against them.
//!
//! Privacy rule for every event: wallets and amounts only (both are already
//! public in the token transfer itself). Anything that identifies a person
//! or an off-chain document goes in as a 32-byte hash, never raw.

use anchor_lang::prelude::*;

/// Which governed config field group changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum ConfigChange {
    /// First write of the config (initialize_config).
    Initialized,
    /// Fee receiver or reserve re-pointed.
    Treasury,
    /// Authority handed over.
    Authority,
}

/// Emitted on every config write.
#[event]
#[derive(Debug)]
pub struct RailsConfigUpdated {
    /// What changed.
    pub change: ConfigChange,
    /// `RailsConfig::fingerprint()` after the write.
    pub new_config_hash: [u8; 32],
    /// Authority that performed the write.
    pub authority: Pubkey,
}

/// Halt switch that moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum Breaker {
    /// Operational pause.
    Pause,
    /// Settlement-asset depeg breaker (06 §2, F4/M5).
    Depeg,
}

/// Emitted when `paused` or `depeg_breaker` flips (R2).
#[event]
#[derive(Debug)]
pub struct BreakerChanged {
    /// Which switch.
    pub breaker: Breaker,
    /// New state (`true` = halted).
    pub engaged: bool,
    /// Hash of the off-chain evidence (price sources, incident note).
    pub evidence_hash: [u8; 32],
    /// Authority that flipped it.
    pub authority: Pubkey,
}

/// Escrow lifecycle step (AE-P3/P4/P10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum EscrowAction {
    /// Buyer cash (`qty × limit`) locked at commit.
    Deposited,
    /// Forfeitable bond locked at commit.
    BondPosted,
    /// Unrevealed ⇒ bond moved to reserve (AE-P4).
    BondForfeited,
    /// Timeout / failed-session refund to the depositor (AE-P10, AC-11).
    Refunded,
}

/// Emitted for every escrow/bond movement (R1).
#[event]
#[derive(Debug)]
pub struct EscrowEvent {
    /// Lifecycle step.
    pub action: EscrowAction,
    /// Session (or facility) the escrow belongs to.
    pub session: Pubkey,
    /// Escrow PDA.
    pub escrow: Pubkey,
    /// Depositor wallet.
    pub owner: Pubkey,
    /// Micro-USDC moved by this step.
    pub amount: u64,
    /// Signer that triggered it (any participant for refunds).
    pub caller: Pubkey,
}

/// Emitted once per settled fill — the DvP cash leg (AE-P8, R2).
/// `to_seller + fee + to_reserve + buyer_refund == escrow_released` always.
#[event]
#[derive(Debug)]
pub struct FillSettled {
    /// Session.
    pub session: Pubkey,
    /// Fill index within the session (receipt PDA seed).
    pub fill_index: u32,
    /// Buyer wallet.
    pub buyer: Pubkey,
    /// Seller wallet (issuer multisig on the ROFR path).
    pub seller: Pubkey,
    /// Security base units delivered.
    pub qty: u64,
    /// Clearing price, micro-USDC per whole unit.
    pub price: u64,
    /// Buyer escrow released for this fill.
    pub escrow_released: u64,
    /// Cash to seller.
    pub to_seller: u64,
    /// Protocol fee.
    pub fee: u64,
    /// Reserve cut.
    pub to_reserve: u64,
    /// Excess escrow returned to buyer (limit above clearing price).
    pub buyer_refund: u64,
    /// `true` when the issuer's ROFR took the fill (AE-P7).
    pub rofr: bool,
    /// Disclosure-policy hash (aggregate-only print discipline).
    pub disclosure_hash: [u8; 32],
}

/// Why cash is being paid out (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum PayoutKind {
    /// Queue-claim sale proceeds to the holder (QC-P3/P4).
    ClaimProceeds,
    /// Facility sweep repaying an advance before the holder (QC-P4).
    FacilityRepayment,
    /// Facility advance out of the fund PDA (LF-P1/P2).
    FacilityAdvance,
    /// Tranche coupon via `credit-math::distribute_cash` (TE-P4).
    Coupon,
}

/// Emitted for every payout to an allowlisted destination (R3).
#[event]
#[derive(Debug)]
pub struct PayoutMade {
    /// Why.
    pub kind: PayoutKind,
    /// Source PDA (escrow, facility fund, distribution vault).
    pub source: Pubkey,
    /// Destination token account (must be in the payout registry).
    pub destination: Pubkey,
    /// Micro-USDC paid.
    pub amount: u64,
    /// Hash of the off-chain reference (claim id, advance id, period).
    pub reference_hash: [u8; 32],
}
