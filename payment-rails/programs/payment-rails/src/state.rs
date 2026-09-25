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
    /// Sole key allowed to open cash sessions and report their outcomes
    /// (reveals, failure). Set to the authority at init; re-pointed to the
    /// auction-engine's session-signer PDA once that program exists (Q1).
    pub operator: Pubkey,
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
        mix(self.operator.as_ref());
        mix(&[u8::from(self.paused), u8::from(self.depeg_breaker)]);
        let mut out = [0u8; 32];
        out[..8].copy_from_slice(&h.to_le_bytes());
        out
    }
}

/// Lifecycle of a cash session. Deliberately tiny: the auction phases live
/// in auction-engine; the rails only need to know "still running" vs
/// "aborted, release everything".
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize, InitSpace)]
pub enum SessionStatus {
    /// Accepting deposits until `commit_end`; escrows locked until an
    /// outcome or the hard deadline.
    Open,
    /// Aborted by the operator (AE-P4 auto-fail, no cross ≥ floor, …).
    /// Every escrow is refundable immediately.
    Failed,
}

/// The cash side of one auction session (AE-P1/P3/P10).
///
/// Everything a refund depends on is snapshotted here at open, so a later
/// config change can never strand or re-route escrowed cash (L5).
#[account]
#[derive(InitSpace)]
pub struct CashSession {
    /// Operator that opened the session (config snapshot) — reports reveals
    /// and failure. Cannot move cash.
    pub operator: Pubkey,
    /// Operator-chosen id; part of the PDA seed.
    pub session_id: u64,
    /// Settlement mint snapshot.
    pub usdc_mint: Pubkey,
    /// Reserve token account snapshot (forfeited bonds go here).
    pub reserve: Pubkey,
    /// Deposits accepted while `now < commit_end`.
    pub commit_end: i64,
    /// Reveals reported while `commit_end <= now < reveal_end`; unrevealed
    /// escrows become forfeitable at `reveal_end`.
    pub reveal_end: i64,
    /// Hard deadline (AE-P10): at/after it every escrow is refundable by
    /// anyone, whatever the session state.
    pub settle_deadline: i64,
    /// Bond every escrow posts (micro-USDC, forfeitable).
    pub bond_amount: u64,
    /// Per-investor cash cap (AE-P3 bid caps; one escrow per owner).
    pub max_cash: u64,
    /// Running / aborted.
    pub status: SessionStatus,
    /// When the operator failed the session (0 while open).
    pub failed_at: i64,
    /// Live escrow count.
    pub open_escrows: u32,
    /// Cash held across live escrows (R4 reconciliation invariant).
    pub total_cash: u64,
    /// Bonds held across live escrows.
    pub total_bonds: u64,
    /// PDA bump.
    pub bump: u8,
}

/// What a refund does with an escrow's bond.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondRoute {
    /// Bond goes back to the owner with the cash.
    ToOwner,
    /// Bond goes to the reserve; cash goes back to the owner (AE-P4).
    ToReserve,
}

impl CashSession {
    /// PDA seed prefix: `["session", operator, session_id_le]`.
    pub const SEED: &'static [u8] = b"session";

    /// Is the bond of an escrow with this reveal flag forfeit at `now`?
    ///
    /// Unrevealed after `reveal_end` ⇒ forfeit (AE-P4). The one exception:
    /// the operator aborted the session before reveals closed, so bidders
    /// never got a full reveal window — no one is penalised for that.
    pub fn bond_forfeit(&self, revealed: bool, now: i64) -> bool {
        if revealed || now < self.reveal_end {
            return false;
        }
        match self.status {
            SessionStatus::Open => true,
            SessionStatus::Failed => self.failed_at >= self.reveal_end,
        }
    }

    /// May this escrow be refunded at `now`, and where does its bond go?
    ///
    /// Refundable when (any): the session failed; the hard deadline passed;
    /// or the escrow was never revealed and reveals are closed (its order is
    /// excluded, so its cash is no longer needed). A revealed escrow in a
    /// running session is awaiting settlement and stays locked.
    pub fn refund_route(&self, revealed: bool, now: i64) -> Result<BondRoute> {
        let refundable = self.status == SessionStatus::Failed
            || now >= self.settle_deadline
            || (!revealed && now >= self.reveal_end);
        require!(refundable, RailsError::DeadlineNotReached);
        Ok(if self.bond_forfeit(revealed, now) {
            BondRoute::ToReserve
        } else {
            BondRoute::ToOwner
        })
    }
}

/// One investor's escrow in one session: cash (`qty × limit`, opaque to the
/// rails so the bid stays sealed) plus the bond. Its tokens sit in a
/// dedicated vault owned by this PDA — never in a shared pool (AE-P10).
#[account]
#[derive(InitSpace)]
pub struct Escrow {
    /// Session this escrow belongs to (`has_one`).
    pub session: Pubkey,
    /// Depositor; the only possible refund destination.
    pub owner: Pubkey,
    /// Vault token account (`has_one`).
    pub vault: Pubkey,
    /// Cash held (micro-USDC).
    pub cash: u64,
    /// Bond held (micro-USDC); 0 once forfeited.
    pub bond: u64,
    /// Reveal reported by the operator.
    pub revealed: bool,
    /// PDA bump.
    pub bump: u8,
}

impl Escrow {
    /// PDA seed prefix: `["escrow", session, owner]`.
    pub const SEED: &'static [u8] = b"escrow";
    /// Vault PDA seed prefix: `["vault", escrow]`.
    pub const VAULT_SEED: &'static [u8] = b"vault";
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
            operator: Pubkey::new_from_array([5; 32]),
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
                operator: Pubkey::new_from_array([9; 32]),
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

    // Schedule used below: commit < 100 ≤ reveal < 200 ≤ settle-wait < 300.
    fn session(status: SessionStatus, failed_at: i64) -> CashSession {
        CashSession {
            operator: Pubkey::new_from_array([5; 32]),
            session_id: 1,
            usdc_mint: Pubkey::new_from_array([2; 32]),
            reserve: Pubkey::new_from_array([4; 32]),
            commit_end: 100,
            reveal_end: 200,
            settle_deadline: 300,
            bond_amount: 10,
            max_cash: 1_000,
            status,
            failed_at,
            open_escrows: 1,
            total_cash: 100,
            total_bonds: 10,
            bump: 255,
        }
    }

    fn locked(s: &CashSession, revealed: bool, now: i64) -> bool {
        s.refund_route(revealed, now).unwrap_err() == RailsError::DeadlineNotReached.into()
    }

    #[test]
    fn open_session_refund_table() {
        let s = session(SessionStatus::Open, 0);
        // Before reveals close nothing is refundable — revealed or not.
        for now in [0, 99, 100, 199] {
            assert!(locked(&s, false, now), "unrevealed locked at {now}");
            assert!(locked(&s, true, now), "revealed locked at {now}");
        }
        // Reveals closed: unrevealed ⇒ cash back, bond forfeit (AE-P4).
        assert_eq!(s.refund_route(false, 200).unwrap(), BondRoute::ToReserve);
        assert_eq!(s.refund_route(false, 10_000).unwrap(), BondRoute::ToReserve);
        // Revealed ⇒ awaiting settlement until the hard deadline …
        assert!(locked(&s, true, 200));
        assert!(locked(&s, true, 299));
        // … then refundable in full (AE-P10, AC-11): not the bidder's fault.
        assert_eq!(s.refund_route(true, 300).unwrap(), BondRoute::ToOwner);
    }

    #[test]
    fn failed_session_refunds_immediately() {
        // Aborted during commit or reveal: everyone whole, no forfeits.
        for failed_at in [50, 150, 199] {
            let s = session(SessionStatus::Failed, failed_at);
            for now in [failed_at, 199, 250, 400] {
                for revealed in [false, true] {
                    assert_eq!(
                        s.refund_route(revealed, now).unwrap(),
                        BondRoute::ToOwner,
                        "failed_at={failed_at} now={now} revealed={revealed}"
                    );
                }
            }
        }
        // Aborted after reveals closed: non-revealers still forfeit.
        let s = session(SessionStatus::Failed, 250);
        assert_eq!(s.refund_route(false, 250).unwrap(), BondRoute::ToReserve);
        assert_eq!(s.refund_route(true, 250).unwrap(), BondRoute::ToOwner);
    }

    #[test]
    fn bond_forfeit_never_hits_revealers() {
        for status in [SessionStatus::Open, SessionStatus::Failed] {
            let s = session(status, 250);
            for now in [0, 199, 200, 300, i64::MAX] {
                assert!(!s.bond_forfeit(true, now));
            }
        }
    }
}
