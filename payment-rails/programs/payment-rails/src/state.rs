//! Account state for the rails.

use anchor_lang::prelude::*;

use crate::error::RailsError;

/// The one global rails configuration.
///
/// Every cash path reads it: the settlement mint all escrows are
/// denominated in, where fees and forfeited bonds go, and the two halt
/// switches. Consumers (auction sessions, facility advances) snapshot the
/// fields they depend on at open — the rails never re-route an in-flight
/// session's cash (L5 snapshot discipline). Updates are governed (Squads
/// 2-of-2) and always evented.
#[account]
#[derive(InitSpace)]
pub struct RailsConfig {
    /// Squads 2-of-2: sole authority for this PDA (SRS §8 protocol authority).
    pub authority: Pubkey,
    /// Settlement mint (USDC). Every escrow, bond and payout uses it.
    pub usdc_mint: Pubkey,
    /// USDC token account receiving protocol fees (AE-P8 fee split).
    pub fee_receiver: Pubkey,
    /// USDC token account receiving forfeited bonds and the reserve cut
    /// (AE-P4, LF-P5 shortfall buffer).
    pub reserve: Pubkey,
    /// Operational pause (authority-set). Blocks new money in; refunds stay
    /// open (AE-P10: refunds are never pausable).
    pub paused: bool,
    /// Settlement-asset depeg breaker (06 §2, F4/M5). Tripped ⇒ no new
    /// settlement at a price denominated in a broken USDC; refunds stay open.
    pub depeg_breaker: bool,
    /// PDA bump.
    pub bump: u8,
}

impl RailsConfig {
    /// PDA seed. One config per program deployment.
    pub const SEED: &'static [u8] = b"rails_config";

    /// Gate for instructions that bring new money into escrow or settle at
    /// a price. Refund paths must NOT call this (AE-P10).
    pub fn ensure_accepting(&self) -> Result<()> {
        require!(!self.paused, RailsError::Paused);
        require!(!self.depeg_breaker, RailsError::DepegBreakerTripped);
        Ok(())
    }

    /// Deterministic fingerprint over config fields for event provenance.
    /// Same FNV-1a placeholder as credit-gate (anchor-lang 1.1 re-exports no
    /// hasher); swaps to SHA-256 with the gate's, and the event field stays
    /// `[u8; 32]` so consumers are unaffected.
    pub fn fingerprint(&self) -> [u8; 32] {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut h = FNV_OFFSET;
        let mut mix = |bytes: &[u8]| {
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(FNV_PRIME);
            }
        };
        mix(self.authority.as_ref());
        mix(self.usdc_mint.as_ref());
        mix(self.fee_receiver.as_ref());
        mix(self.reserve.as_ref());
        mix(&[u8::from(self.paused), u8::from(self.depeg_breaker)]);
        let mut out = [0u8; 32];
        out[..8].copy_from_slice(&h.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RailsConfig {
        RailsConfig {
            authority: Pubkey::new_from_array([1; 32]),
            usdc_mint: Pubkey::new_from_array([2; 32]),
            fee_receiver: Pubkey::new_from_array([3; 32]),
            reserve: Pubkey::new_from_array([4; 32]),
            paused: false,
            depeg_breaker: false,
            bump: 255,
        }
    }

    #[test]
    fn accepting_only_when_both_switches_clear() {
        assert!(cfg().ensure_accepting().is_ok());

        let paused = RailsConfig {
            paused: true,
            ..cfg()
        };
        assert_eq!(
            paused.ensure_accepting().unwrap_err(),
            RailsError::Paused.into()
        );

        let depegged = RailsConfig {
            depeg_breaker: true,
            ..cfg()
        };
        assert_eq!(
            depegged.ensure_accepting().unwrap_err(),
            RailsError::DepegBreakerTripped.into()
        );
    }

    #[test]
    fn fingerprint_tracks_every_governed_field() {
        let base = cfg().fingerprint();
        assert_eq!(base, cfg().fingerprint(), "deterministic");
        let variants = [
            RailsConfig {
                authority: Pubkey::new_from_array([9; 32]),
                ..cfg()
            },
            RailsConfig {
                usdc_mint: Pubkey::new_from_array([9; 32]),
                ..cfg()
            },
            RailsConfig {
                fee_receiver: Pubkey::new_from_array([9; 32]),
                ..cfg()
            },
            RailsConfig {
                reserve: Pubkey::new_from_array([9; 32]),
                ..cfg()
            },
            RailsConfig {
                paused: true,
                ..cfg()
            },
            RailsConfig {
                depeg_breaker: true,
                ..cfg()
            },
        ];
        for v in variants {
            assert_ne!(v.fingerprint(), base);
        }
        // The bump is derivation metadata, not governed state.
        let rebump = RailsConfig { bump: 1, ..cfg() };
        assert_eq!(rebump.fingerprint(), base);
    }
}
