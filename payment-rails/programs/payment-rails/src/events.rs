//! Event families. Every cash movement and every state change emits exactly
//! one event from these families (a refund that also forfeits a bond emits
//! one per movement); the R4 ledger reconciles escrow balances against them.
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
    /// Session operator re-pointed (appended R1).
    Operator,
    /// Fee / reserve bps changed (appended R2).
    Fees,
    /// Payout registrar re-pointed (appended R3).
    Registrar,
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

/// Emitted when `paused` or `depeg_breaker` flips (`set_breaker`).
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
    /// Operator reported the escrow's bid revealed (no cash moves; amount 0).
    Revealed,
    /// Seller asset locked at commit (amount in asset base units, R2).
    AssetDeposited,
    /// Unsold seller asset returned (amount in asset base units, R2).
    AssetReturned,
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

/// Emitted once per settled fill — the DvP settlement (AE-P7/P8, R2).
/// `to_seller + fee + to_reserve == gross` always, and `qty` of the asset
/// moved seller → buyer in the same instruction.
#[event]
#[derive(Debug)]
pub struct FillSettled {
    /// Session.
    pub session: Pubkey,
    /// Fill index within the session (receipt PDA seed).
    pub fill_index: u32,
    /// Buyer wallet (issuer on the ROFR path).
    pub buyer: Pubkey,
    /// Seller wallet.
    pub seller: Pubkey,
    /// Asset base units delivered.
    pub qty: u64,
    /// Price, micro-USDC per whole asset unit.
    pub price: u64,
    /// Buyer cash released for this fill.
    pub gross: u64,
    /// Cash to seller.
    pub to_seller: u64,
    /// Protocol fee.
    pub fee: u64,
    /// Reserve cut.
    pub to_reserve: u64,
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

/// Cash-session lifecycle step (R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum SessionAction {
    /// Session opened with snapshotted deadlines.
    Opened,
    /// Operator aborted the session; all escrows refundable.
    Failed,
    /// Operator closed settlement; remaining escrow refundable (R2).
    Settled,
}

/// Emitted when a cash session opens or fails (R1).
#[event]
#[derive(Debug)]
pub struct SessionEvent {
    /// Lifecycle step.
    pub action: SessionAction,
    /// Session PDA.
    pub session: Pubkey,
    /// Operator.
    pub operator: Pubkey,
    /// Hard refund deadline (AE-P10).
    pub settle_deadline: i64,
    /// Cluster time of the step.
    pub ts: i64,
}

/// Payout-registry step (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum DestinationAction {
    /// First admission of a wallet.
    Registered,
    /// Attestation refreshed / token account re-pointed; re-activates.
    Renewed,
    /// Revoked (block-list precedence).
    Revoked,
}

/// Emitted on every payout-registry write (R3). Hash only — no PII.
#[event]
#[derive(Debug)]
pub struct DestinationEvent {
    /// Step.
    pub action: DestinationAction,
    /// Wallet.
    pub owner: Pubkey,
    /// Registered USDC token account.
    pub token_account: Pubkey,
    /// Attestation reference hash.
    pub attestation_ref_hash: [u8; 32],
    /// Expiry.
    pub expires_at: i64,
    /// Registrar that performed it.
    pub registrar: Pubkey,
}

/// Facility lifecycle step (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum FacilityAction {
    /// Facility + vault created.
    Opened,
    /// Cash added to the vault.
    Funded,
    /// An advance was fully repaid and closed.
    AdvanceClosed,
}

/// Emitted for facility steps that are not payouts (R3).
#[event]
#[derive(Debug)]
pub struct FacilityEvent {
    /// Step.
    pub action: FacilityAction,
    /// Facility PDA.
    pub facility: Pubkey,
    /// Micro-USDC involved (0 for `Opened`).
    pub amount: u64,
    /// Signer.
    pub actor: Pubkey,
}
