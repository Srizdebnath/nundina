//! Error and rounding-direction types. `#[repr(u32)]` values are stable —
//! programs embed them in typed rejection reasons (CG-P7 discipline).

/// Fixed arithmetic error set for all `credit-math` functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MathError {
    /// Division by zero (denominator was 0).
    DivisionByZero = 1,
    /// Result exceeded u128.
    Overflow = 2,
    /// Input violated a structural invariant (e.g. senior > total).
    InvalidInput = 3,
    /// Underflow in a checked subtraction (loss exceeded the buffer being hit).
    Underflow = 4,
    /// Sum of parts would exceed the whole (allocation over-commit).
    OverAllocation = 5,
}

/// Which way a rounding site moves, relative to the initiator (lib docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundDirection {
    /// Truncate (default against-initiator floor for claims on the protocol).
    Floor,
    /// Round up (distributor-initiated payouts; dust line absorbs residue).
    Ceil,
    /// Explicit alias of `Floor` at call sites, for audit readability.
    AgainstInitiator,
}
