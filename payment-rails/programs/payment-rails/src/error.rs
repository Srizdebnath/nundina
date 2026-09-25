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
    }
}
