//! Typed error set for the rails. Wire-stable: variants are append-only
//! (off-chain crank/UI map codes to reasons), so later phases add at the end.

use anchor_lang::prelude::*;
use credit_math::MathError;

#[error_code]
pub enum RailsError {
    // ── config / governance (R0) ─────────────────────────────────────────
    /// Signer is not the rails config authority.
    #[msg("Signer is not the rails config authority")]
    Unauthorized,
    /// Signer is not the program's upgrade authority (init gate).
    #[msg("Signer is not the program upgrade authority")]
    NotUpgradeAuthority,
    /// Settlement mint is not 6-decimal (credit-math is micro-USDC).
    #[msg("Settlement mint must have 6 decimals")]
    InvalidMintDecimals,
    /// A treasury token account is not denominated in the settlement mint.
    #[msg("Token account mint does not match the settlement mint")]
    MintMismatch,
    /// Fee receiver and reserve must be distinct accounts.
    #[msg("Fee receiver and reserve must be distinct accounts")]
    DuplicateTreasuryAccount,
    /// Rails are paused; new money in / settlement blocked.
    #[msg("Rails are paused")]
    Paused,
    /// Settlement-asset depeg breaker is tripped.
    #[msg("Settlement-asset depeg breaker is tripped")]
    DepegBreakerTripped,

    // ── escrow / refunds (R1) ────────────────────────────────────────────
    /// Refund/forfeit attempted before the phase deadline (AE-P10).
    #[msg("Deadline has not been reached")]
    DeadlineNotReached,
    /// Escrow already refunded, forfeited or settled.
    #[msg("Escrow is already closed")]
    EscrowClosed,

    // ── DvP cash leg (R2) ────────────────────────────────────────────────
    /// Fill receipt already exists for this (session, fill) (idempotency).
    #[msg("Fill already settled")]
    FillAlreadySettled,
    /// Cash legs would not sum exactly to escrowed cash.
    #[msg("Settlement does not conserve escrowed cash")]
    ConservationViolated,

    // ── payout router (R3) ───────────────────────────────────────────────
    /// Destination is not a registered, attested payout wallet.
    #[msg("Payout destination is not allowlisted")]
    DestinationNotAllowlisted,

    // ── shared math ──────────────────────────────────────────────────────
    /// `credit-math` division by zero.
    #[msg("Math: division by zero")]
    MathDivisionByZero,
    /// `credit-math` overflow, or a result that does not fit a token amount.
    #[msg("Math: overflow")]
    MathOverflow,
    /// `credit-math` rejected its inputs.
    #[msg("Math: invalid input")]
    MathInvalidInput,
    /// `credit-math` checked subtraction underflowed.
    #[msg("Math: underflow")]
    MathUnderflow,
    /// `credit-math` allocation would exceed the whole.
    #[msg("Math: over-allocation")]
    MathOverAllocation,

    // ── escrow / sessions (R1, appended) ─────────────────────────────────
    /// Signer is not the config/session operator.
    #[msg("Signer is not the session operator")]
    NotOperator,
    /// Session deadlines must satisfy now < commit_end < reveal_end < settle_deadline.
    #[msg("Session schedule is not strictly increasing from now")]
    InvalidSchedule,
    /// Amount must be non-zero.
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    /// Deposit above the session's per-investor cash cap (AE-P3).
    #[msg("Deposit exceeds the per-investor cash cap")]
    CashCapExceeded,
    /// Session was failed; only refunds remain.
    #[msg("Session is not open")]
    SessionNotOpen,
    /// Action outside its phase window (deposit after commit, reveal outside reveal window).
    #[msg("Outside the permitted phase window")]
    OutsideWindow,
    /// Reveal already reported for this escrow.
    #[msg("Escrow already revealed")]
    AlreadyRevealed,
    /// Bond is not forfeitable (revealed, window open, or already taken).
    #[msg("Bond is not forfeitable")]
    BondNotForfeitable,
    /// Refund destination is not a token account of the escrow owner.
    #[msg("Refund destination is not owned by the escrow owner")]
    NotEscrowOwner,
    /// Bond destination is not the session's snapshotted reserve.
    #[msg("Account is not the session reserve")]
    ReserveMismatch,

    // ── DvP settlement (R2, appended) ────────────────────────────────────
    /// Fee or reserve bps above the hard on-chain ceiling.
    #[msg("Fee or reserve bps above the hard ceiling")]
    FeeTooHigh,
    /// Escrow is on the wrong side for this operation.
    #[msg("Escrow is on the wrong side for this operation")]
    WrongSide,
    /// Only revealed escrows can be filled.
    #[msg("Escrow has not been revealed")]
    NotRevealed,
    /// Fill exceeds what the escrow holds (buyer cash or seller asset).
    #[msg("Fill exceeds the escrowed amount")]
    InsufficientEscrow,
    /// Token account/mint is not the session's traded asset.
    #[msg("Account is not the session asset")]
    AssetMismatch,
    /// Signer is not the session's ROFR authority, or ROFR is disabled.
    #[msg("Signer is not the session ROFR authority")]
    NotRofrAuthority,
    /// Account is not the session's snapshotted fee receiver.
    #[msg("Account is not the session fee receiver")]
    FeeReceiverMismatch,

    // ── payout router (R3, appended) ─────────────────────────────────────
    /// Payout destination was revoked (block-list precedence).
    #[msg("Payout destination has been revoked")]
    DestinationRevoked,
    /// Payout destination's attestation has expired.
    #[msg("Payout destination attestation has expired")]
    DestinationExpired,
    /// Signer is not the payout registrar.
    #[msg("Signer is not the payout registrar")]
    NotRegistrar,
    /// Facility vault can't cover the advance.
    #[msg("Facility has insufficient funds")]
    FacilityUnderfunded,
    /// Facility rate above the hard ceiling.
    #[msg("Facility rate above the hard ceiling")]
    RateTooHigh,
    /// Nothing is owed on this advance.
    #[msg("Nothing is owed on this advance")]
    NothingOwed,
    /// Signer is not the facility curator.
    #[msg("Signer is not the facility curator")]
    NotCurator,

    // ── token leg (R5, appended) ─────────────────────────────────────────
    /// Ask asset vault is still frozen (NPCS: DefaultAccountState=Frozen);
    /// the gate must thaw it between `open_asset_vault` and `deposit_ask`.
    #[msg("Asset vault is frozen; the gate must thaw it before deposit_ask")]
    AssetVaultFrozen,
    /// Refund destination (`owner_asset`) is frozen by the asset's freeze
    /// authority (#90). Pass another thawed asset account of the same
    /// owner, or have the issuer thaw this one; nothing has moved.
    #[msg("Refund asset destination is frozen; pass a thawed account of the owner")]
    RefundAssetDestinationFrozen,
    /// Refund destination (`owner_token` / `owner_usdc`) is frozen by the
    /// settlement mint's freeze authority (#90). Nothing has moved.
    #[msg("Refund USDC destination is frozen; pass a thawed account of the owner")]
    RefundUsdcDestinationFrozen,
    /// The seller's asset vault holds less than the fill's `qty`: it was
    /// drained (e.g. by the asset's permanent delegate) after deposit_ask
    /// (#90). The fill cannot deliver; refund_ask returns what is held.
    #[msg("Seller asset vault holds less than the fill quantity")]
    EscrowDrained,
    /// `advance_rate_bps` above the H1 hard ceiling,
    /// `credit_math::MAX_ADVANCE_RATE_BPS` (50%, #94).
    #[msg("Advance rate above the hard ceiling")]
    AdvanceRateTooHigh,
    /// `amount` above `collateral_value × advance_rate_bps / 10⁴` (#94).
    #[msg("Advance exceeds collateral value × advance rate")]
    AdvanceExceedsRate,
    /// A fill's lot list is longer than `MAX_LOTS_PER_FILL` (#88).
    #[msg("Too many lots in one fill")]
    TooManyLots,
    /// Keeper policy values outside their bounds (#127): SLA in
    /// `1..=MAX_KEEPER_SLA_SECONDS`, `0 < escalation_reward ≤ min_bond`.
    #[msg("Keeper policy out of bounds")]
    KeeperPolicyOutOfBounds,
    /// Keeper bond below the policy minimum (#127).
    #[msg("Keeper bond below the minimum")]
    KeeperBondTooLow,
    /// Escalation before the session's keeper SLA ran out (#127).
    #[msg("Keeper SLA has not run out")]
    KeeperSlaNotDue,
    /// Keeper still holds live session assignments (#127).
    #[msg("Keeper has live assignments")]
    KeeperAssigned,
    /// Assignment released while escrows are still open (#127).
    #[msg("Session still has open escrows")]
    SessionStillOpen,
    /// The keeper assignment belongs to another session (#127).
    #[msg("Keeper assignment is for another session")]
    KeeperSessionMismatch,
}

impl From<MathError> for RailsError {
    fn from(e: MathError) -> Self {
        match e {
            MathError::DivisionByZero => RailsError::MathDivisionByZero,
            MathError::Overflow => RailsError::MathOverflow,
            MathError::InvalidInput => RailsError::MathInvalidInput,
            MathError::Underflow => RailsError::MathUnderflow,
            MathError::OverAllocation => RailsError::MathOverAllocation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_errors_map_one_to_one() {
        let all = [
            MathError::DivisionByZero,
            MathError::Overflow,
            MathError::InvalidInput,
            MathError::Underflow,
            MathError::OverAllocation,
        ];
        let mapped: Vec<u32> = all.iter().map(|e| RailsError::from(*e) as u32).collect();
        let mut dedup = mapped.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), all.len(), "no two math errors collapse");
    }

    #[test]
    fn r0_codes_are_wire_stable() {
        // Append-only discipline: pin the first codes so a reorder fails CI.
        assert_eq!(u32::from(RailsError::Unauthorized), 6000);
        assert_eq!(u32::from(RailsError::DepegBreakerTripped), 6006);
        assert_eq!(u32::from(RailsError::MathOverAllocation), 6016);
        assert_eq!(u32::from(RailsError::NotOperator), 6017);
        assert_eq!(u32::from(RailsError::ReserveMismatch), 6026);
        assert_eq!(u32::from(RailsError::FeeTooHigh), 6027);
        assert_eq!(u32::from(RailsError::FeeReceiverMismatch), 6033);
        assert_eq!(u32::from(RailsError::DestinationRevoked), 6034);
        assert_eq!(u32::from(RailsError::AssetVaultFrozen), 6041);
        assert_eq!(u32::from(RailsError::RefundAssetDestinationFrozen), 6042);
        assert_eq!(u32::from(RailsError::TooManyLots), 6047);
    }
}
