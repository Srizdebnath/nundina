//! Account state for the rails.

use anchor_lang::prelude::*;

use crate::error::RailsError;

/// Hard ceiling on the protocol fee (bps of fill gross): **zero** (#147).
/// doc 07 AE-3 forbids a protocol-collected per-trade fee, so no
/// governance action can set one; the `fee` line of every split is 0.
/// Changing this is a legal decision (doc 07 L2), not a parameter tweak.
pub const MAX_FEE_BPS: u16 = 0;

/// `fee_bps` within [`MAX_FEE_BPS`] (one place, so the ceiling can move
/// without rewriting every check; clippy sees `x <= 0` as degenerate).
#[allow(clippy::absurd_extreme_comparisons)]
pub const fn fee_within_cap(fee_bps: u16) -> bool {
    fee_bps <= MAX_FEE_BPS
}
/// Hard ceiling on the reserve cut (bps of fill gross).
pub const MAX_RESERVE_BPS: u16 = 200;
/// Basis-point denominator.
pub const BPS: u128 = 10_000;
/// Hard ceiling on a facility's APR (bps). Pilot runs 12% (1_200).
pub const MAX_RATE_BPS: u64 = 5_000;

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
    /// Payout-registry key (R3): the only key that can admit, renew or
    /// revoke payout destinations. Stands in for the SAS attester until the
    /// gate's attestation read is on-chain (Q2). Set to the authority at init.
    pub registrar: Pubkey,
    /// Protocol fee on each fill's gross, bps (≤ `MAX_FEE_BPS`).
    pub fee_bps: u16,
    /// Reserve cut on each fill's gross, bps (≤ `MAX_RESERVE_BPS`).
    pub reserve_bps: u16,
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
    /// FNV-1a-64 in bytes 0..8, bytes 8..32 zero: not cryptographic.
    /// credit-gate has moved to SHA-256 via `solana-sha256-hasher` (#49).
    /// The rails keep FNV until their next deliberate upgrade, because
    /// swapping it changes the live devnet program's event values. The event
    /// field stays `[u8; 32]` either way.
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
        mix(self.registrar.as_ref());
        mix(&self.fee_bps.to_le_bytes());
        mix(&self.reserve_bps.to_le_bytes());
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
    /// All fills settled (operator). Remaining cash, asset and bonds are
    /// refundable immediately; unrevealed bonds still forfeit (R2).
    Settled,
}

/// Which side of the book an escrow is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize, InitSpace)]
pub enum Side {
    /// Holds cash (`qty × limit`) + bond.
    Buy,
    /// Holds the traded asset + a USDC bond (AE-P3 seller escrow).
    Sell,
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
    /// Reserve token account snapshot (forfeited bonds, reserve cut).
    pub reserve: Pubkey,
    /// Fee token account snapshot.
    pub fee_receiver: Pubkey,
    /// Traded asset (NPCS or a queue-claim token). Sellers escrow it; each
    /// fill delivers it to the buyer atomically with the cash leg (DvP).
    pub asset_mint: Pubkey,
    /// Asset decimals: prices are micro-USDC per whole asset unit.
    pub asset_decimals: u8,
    /// Fee bps snapshot.
    pub fee_bps: u16,
    /// Reserve bps snapshot.
    pub reserve_bps: u16,
    /// Issuer ROFR authority (AE-P7); `Pubkey::default()` = no ROFR.
    pub rofr_authority: Pubkey,
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
    /// Asset held across live sell escrows.
    pub total_asset: u64,
    /// Fills settled so far.
    pub fills_settled: u32,
    /// Asset delivered across settled fills.
    pub volume_settled: u64,
    /// Cash (gross) paid across settled fills.
    pub gross_settled: u64,
    /// Micro-USDC that refunds recorded but did not find in a vault (#89):
    /// the sum of every `CashShortfall`. A refund still completes and stops
    /// tracking the recorded amount, so without this the loss would leave
    /// the books. Non-zero means escrowed cash was taken (e.g. by a
    /// settlement-mint clawback) and reconciliation must not report clean.
    pub total_cash_shortfall: u64,
    /// Asset base units that `refund_ask` recorded but did not find (#89):
    /// the sum of every `AssetShortfall` (e.g. a permanent-delegate pull).
    pub total_asset_shortfall: u64,
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
            SessionStatus::Open | SessionStatus::Settled => true,
            SessionStatus::Failed => self.failed_at >= self.reveal_end,
        }
    }

    /// May this escrow be refunded at `now`, and where does its bond go?
    ///
    /// Refundable when (any): the session failed or finished settling; the
    /// hard deadline passed;
    /// or the escrow was never revealed and reveals are closed (its order is
    /// excluded, so its cash is no longer needed). A revealed escrow in a
    /// running session is awaiting settlement and stays locked.
    pub fn refund_route(&self, revealed: bool, now: i64) -> Result<BondRoute> {
        let refundable = self.status != SessionStatus::Open
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

/// How a refund drains one escrow's USDC vault (AE-P10, #62, #89).
///
/// The forfeit (bond to the reserve) is paid first, up to what is held;
/// everything else held, donated dust included, goes to the owner. A vault
/// holding less than `recorded` still drains: the gap is the shortfall the
/// caller books on the session. `to_reserve + to_owner == held`, always.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaultDrain {
    /// → reserve (the forfeited bond, capped at what is held).
    pub to_reserve: u64,
    /// → owner (the rest of the vault).
    pub to_owner: u64,
    /// `recorded − held` when the vault was drained by someone else.
    pub shortfall: u64,
}

impl VaultDrain {
    /// Split a vault holding `held` that the books say holds `recorded`,
    /// of which `forfeit` is owed to the reserve.
    pub fn compute(held: u64, recorded: u64, forfeit: u64) -> VaultDrain {
        let to_reserve = forfeit.min(held);
        VaultDrain {
            to_reserve,
            to_owner: held - to_reserve,
            shortfall: recorded.saturating_sub(held),
        }
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
    /// Buy (cash) or sell (asset) escrow.
    pub side: Side,
    /// Cash held (micro-USDC); always 0 on the sell side.
    pub cash: u64,
    /// Asset held (base units); always 0 on the buy side.
    pub asset: u64,
    /// Sell side: asset vault (`["asset_vault", escrow]`); default on buys.
    pub asset_vault: Pubkey,
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
    /// Vault PDA seed prefix: `["vault", escrow]` (USDC: cash and/or bond).
    pub const VAULT_SEED: &'static [u8] = b"vault";
    /// Asset vault PDA seed prefix: `["asset_vault", escrow]` (sell side).
    pub const ASSET_VAULT_SEED: &'static [u8] = b"asset_vault";
}

/// Idempotency + audit record for one settled fill (AE-P8). Its PDA is
/// created by `settle_fill`, so replaying a fill index cannot pay twice.
#[account]
#[derive(InitSpace)]
pub struct FillReceipt {
    /// Session.
    pub session: Pubkey,
    /// Fill index (seed).
    pub fill_index: u32,
    /// Buyer wallet (issuer on the ROFR path).
    pub buyer: Pubkey,
    /// Seller wallet.
    pub seller: Pubkey,
    /// Asset delivered (base units).
    pub qty: u64,
    /// Price, micro-USDC per whole asset unit.
    pub price: u64,
    /// The split of this fill's cash.
    pub split: FillSplit,
    /// Issuer ROFR fill (AE-P7).
    pub rofr: bool,
    /// Cluster time of settlement.
    pub settled_at: i64,
    /// PDA bump.
    pub bump: u8,
}

impl FillReceipt {
    /// PDA seed prefix: `["fill", session, fill_index_le]`.
    pub const SEED: &'static [u8] = b"fill";
}

/// Cash split of one fill. `to_seller + fee + to_reserve == gross` always.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize, InitSpace)]
pub struct FillSplit {
    /// Buyer cash released: `ceil(qty × price / 10^asset_decimals)`.
    pub gross: u64,
    /// Protocol fee: `floor(gross × fee_bps / 10⁴)`.
    pub fee: u64,
    /// Reserve cut: `floor(gross × reserve_bps / 10⁴)`.
    pub to_reserve: u64,
    /// Seller proceeds: the remainder.
    pub to_seller: u64,
}

impl FillSplit {
    /// Split one fill's cash with `credit-math`.
    ///
    /// Rounding: the buyer (the party taking the fill) pays the ceiling, so
    /// the seller is never short-changed by truncation; fee and reserve are
    /// floored, and the seller absorbs the remainder. Nothing is ever lost
    /// or created: the four legs sum exactly to `gross`.
    pub fn compute(
        qty: u64,
        price: u64,
        asset_decimals: u8,
        fee_bps: u16,
        reserve_bps: u16,
    ) -> Result<FillSplit> {
        require!(qty > 0 && price > 0, RailsError::ZeroAmount);
        require!(
            fee_within_cap(fee_bps) && reserve_bps <= MAX_RESERVE_BPS,
            RailsError::FeeTooHigh
        );
        let unit = 10u128
            .checked_pow(u32::from(asset_decimals))
            .ok_or(RailsError::MathOverflow)?;
        // u64 × u64 always fits u128.
        let notional = u128::from(qty) * u128::from(price);
        let gross = credit_math::ceil_div(notional, unit);
        let fee =
            credit_math::mul_div(gross, u128::from(fee_bps), BPS).map_err(RailsError::from)?;
        let to_reserve =
            credit_math::mul_div(gross, u128::from(reserve_bps), BPS).map_err(RailsError::from)?;
        let to_seller = gross
            .checked_sub(fee + to_reserve)
            .ok_or(RailsError::MathUnderflow)?;
        let narrow = |v: u128| u64::try_from(v).map_err(|_| error!(RailsError::MathOverflow));
        Ok(FillSplit {
            gross: narrow(gross)?,
            fee: narrow(fee)?,
            to_reserve: narrow(to_reserve)?,
            to_seller: narrow(to_seller)?,
        })
    }
}

// ── R3 · payout router ──────────────────────────────────────────────────

/// An attested, allowlisted payout wallet (R3, "private" rails).
///
/// The only place any payout path is allowed to send cash to a person. No
/// PII: the off-chain KYC/attestation record appears only as a hash.
#[account]
#[derive(InitSpace)]
pub struct PayoutDestination {
    /// Wallet this entry belongs to (PDA seed).
    pub owner: Pubkey,
    /// The one USDC token account payouts may go to (owned by `owner`).
    pub token_account: Pubkey,
    /// Hash of the off-chain attestation / KYC reference. Never raw data.
    /// Supplied by the registrar and **not checked** against any SAS
    /// attestation: the registrar key vouches for it (#52).
    pub attestation_ref_hash: [u8; 32],
    /// Registrar that admitted / last renewed it.
    pub registrar: Pubkey,
    /// Payouts rejected at/after this time (attestation expiry, H4).
    pub expires_at: i64,
    /// `false` once revoked (block-list precedence, CG-P5).
    pub active: bool,
    /// PDA bump.
    pub bump: u8,
}

impl PayoutDestination {
    /// PDA seed prefix: `["payout", owner]`.
    pub const SEED: &'static [u8] = b"payout";

    /// Typed payability check for a payout of cash into `token_account` at
    /// `now`: revoked, expired, or a token account other than the
    /// registered one are each rejected with their own reason.
    pub fn ensure_payable(&self, token_account: &Pubkey, now: i64) -> Result<()> {
        require!(self.active, RailsError::DestinationRevoked);
        require!(now < self.expires_at, RailsError::DestinationExpired);
        require_keys_eq!(
            *token_account,
            self.token_account,
            RailsError::DestinationNotAllowlisted
        );
        Ok(())
    }
}

/// A liquidity-facility fund (LF-P1/P2): USDC in a program-owned vault,
/// lent out as advances to registered destinations only.
#[account]
#[derive(InitSpace)]
pub struct Facility {
    /// Curator key (the liquidity-facility engine's signer; underwriting —
    /// advance rate vs `MARK_PROTECTIVE` — happens there, not here).
    pub curator: Pubkey,
    /// Curator-chosen id; part of the PDA seed.
    pub facility_id: u64,
    /// Settlement mint snapshot.
    pub usdc_mint: Pubkey,
    /// Vault token account (`["facility_vault", facility]`).
    pub vault: Pubkey,
    /// Simple APR in bps (pilot: 1_200 = 12%), ≤ `MAX_RATE_BPS`.
    pub rate_bps: u64,
    /// Principal currently lent out across live advances.
    pub principal_outstanding: u64,
    /// Lifetime cash in from funders.
    pub total_funded: u64,
    /// Lifetime interest collected.
    pub interest_collected: u64,
    /// PDA bump.
    pub bump: u8,
}

impl Facility {
    /// PDA seed prefix: `["facility", curator, facility_id_le]`.
    pub const SEED: &'static [u8] = b"facility";
    /// Vault PDA seed prefix: `["facility_vault", facility]`.
    pub const VAULT_SEED: &'static [u8] = b"facility_vault";
}

/// One borrower's advance from one facility (LF-P1/P6).
#[account]
#[derive(InitSpace)]
pub struct Advance {
    /// Facility (`has_one`).
    pub facility: Pubkey,
    /// Borrower (the pledging claim holder).
    pub borrower: Pubkey,
    /// Hash of the pledged collateral reference (claim / lot id).
    pub collateral_ref_hash: [u8; 32],
    /// Collateral mark the advance was sized against (micro-USDC), as
    /// supplied by the curator (#94). There is no mark engine yet, so the
    /// rails cannot verify it; they bound the advance by it.
    pub collateral_value: u64,
    /// Advance rate the curator applied, in bps; at most the H1 ceiling
    /// `credit_math::MAX_ADVANCE_RATE_BPS` (#94). Original principal ≤
    /// `collateral_value × advance_rate_bps / 10⁴`.
    pub advance_rate_bps: u64,
    /// Principal still owed.
    pub principal: u64,
    /// Interest accrued and not yet paid.
    pub interest: u64,
    /// Last accrual time.
    pub last_accrual: i64,
    /// PDA bump.
    pub bump: u8,
}

/// How a repayment was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repayment {
    /// To accrued interest (paid first).
    pub interest: u64,
    /// To principal.
    pub principal: u64,
}

impl Repayment {
    /// Total applied.
    pub fn total(&self) -> u64 {
        self.interest + self.principal
    }
}

impl Advance {
    /// PDA seed prefix: `["advance", facility, borrower]`.
    pub const SEED: &'static [u8] = b"advance";

    /// H1 (doc 06 §11, SRS 08 LF-P1, #94): the most that may be advanced
    /// against `collateral_value` at `advance_rate_bps`, i.e.
    /// `floor(collateral_value × advance_rate_bps / 10⁴)` (floored against
    /// the borrower). Rejects a rate above `credit_math::MAX_ADVANCE_RATE_BPS`
    /// — the hard ceiling no curator parameter can exceed.
    pub fn max_advance(collateral_value: u64, advance_rate_bps: u64) -> Result<u64> {
        credit_math::check_advance_rate(advance_rate_bps)
            .map_err(|_| error!(RailsError::AdvanceRateTooHigh))?;
        let cap = credit_math::mul_div(
            u128::from(collateral_value),
            u128::from(advance_rate_bps),
            u128::from(credit_math::BPS_DENOMINATOR),
        )
        .map_err(RailsError::from)?;
        // rate ≤ 50%, so cap ≤ collateral_value/2 always fits.
        Ok(u64::try_from(cap).map_err(|_| RailsError::MathOverflow)?)
    }

    /// Accrue simple interest on outstanding principal up to `now`
    /// (`credit_math::accrue_interest`, floored). Time never runs backwards.
    pub fn accrue(&mut self, rate_bps: u64, now: i64) -> Result<()> {
        let dt = u64::try_from(now.saturating_sub(self.last_accrual)).unwrap_or(0);
        let i = credit_math::accrue_interest(u128::from(self.principal), rate_bps, dt)
            .map_err(RailsError::from)?;
        let i = u64::try_from(i).map_err(|_| error!(RailsError::MathOverflow))?;
        self.interest = self
            .interest
            .checked_add(i)
            .ok_or(RailsError::MathOverflow)?;
        self.last_accrual = self.last_accrual.max(now);
        Ok(())
    }

    /// Total owed right now (call `accrue` first).
    pub fn owed(&self) -> u64 {
        self.principal.saturating_add(self.interest)
    }

    /// Apply up to `amount`: interest first, then principal. Never takes
    /// more than is owed; the caller moves exactly `total()` of cash.
    pub fn apply(&mut self, amount: u64) -> Repayment {
        let interest = amount.min(self.interest);
        let principal = (amount - interest).min(self.principal);
        self.interest -= interest;
        self.principal -= principal;
        Repayment {
            interest,
            principal,
        }
    }
}

/// Borsh mirror of `credit_math::WaterfallParams` for one coupon period
/// (TE-P4). All amounts micro-USDC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize, InitSpace)]
pub struct CouponParams {
    /// 1 · fees & expenses.
    pub fees_due: u64,
    /// 2 · senior coupon.
    pub senior_coupon_due: u64,
    /// 3 · reserve refilled toward this target …
    pub reserve_target: u64,
    /// … from this current tranche-reserve level (tranche-engine ledger).
    pub reserve_current: u64,
    /// 4 · senior amortization.
    pub senior_amort_due: u64,
    /// 5 · junior coupon.
    pub junior_coupon_due: u64,
}

/// Where one period's cash went. `fees + reserve + senior + junior == cash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize, InitSpace)]
pub struct CouponSplit {
    /// Cash distributed.
    pub cash: u64,
    /// → fee receiver.
    pub fees: u64,
    /// → reserve (top-up line).
    pub reserve: u64,
    /// → senior holder (coupon + amortization).
    pub senior: u64,
    /// → junior holder (coupon + residual).
    pub junior: u64,
    /// Senior coupon shortfall (defers, never a default event).
    pub unpaid_senior_coupon: u64,
}

impl CouponSplit {
    /// Run the waterfall (`credit_math::distribute_cash`) and group its six
    /// lines by destination. Fails unless the lines sum exactly to `cash`.
    pub fn compute(cash: u64, p: &CouponParams) -> Result<CouponSplit> {
        let w = credit_math::distribute_cash(
            u128::from(cash),
            &credit_math::WaterfallParams {
                fees_due: u128::from(p.fees_due),
                senior_coupon_due: u128::from(p.senior_coupon_due),
                reserve_target: u128::from(p.reserve_target),
                reserve_current: u128::from(p.reserve_current),
                senior_amort_due: u128::from(p.senior_amort_due),
                junior_coupon_due: u128::from(p.junior_coupon_due),
            },
        )
        .map_err(RailsError::from)?;
        require!(
            w.total_distributed().map_err(RailsError::from)? == u128::from(cash),
            RailsError::ConservationViolated
        );
        // Every line ≤ cash ≤ u64::MAX, so these narrowings cannot fail.
        let n = |v: u128| u64::try_from(v).map_err(|_| error!(RailsError::MathOverflow));
        let split = CouponSplit {
            cash,
            fees: n(w.paid_fees)?,
            reserve: n(w.paid_reserve)?,
            senior: n(w.paid_senior_coupon + w.paid_senior_amort)?,
            junior: n(w.paid_junior_coupon + w.junior_residual)?,
            unpaid_senior_coupon: n(w.unpaid_senior_coupon)?,
        };
        require!(
            u128::from(split.fees)
                + u128::from(split.reserve)
                + u128::from(split.senior)
                + u128::from(split.junior)
                == u128::from(cash),
            RailsError::ConservationViolated
        );
        Ok(split)
    }
}

// ── #127 · bonded keepers (doc 06 §8) ─────────────────────────────────

/// Ceiling on a keeper SLA: 30 days. A sanity bound, not a policy number.
pub const MAX_KEEPER_SLA_SECONDS: i64 = 30 * 86_400;

/// Keeper terms, set by the config authority. Snapshotted into every
/// assignment, so a later change never moves a live session's SLA (L5).
#[account]
#[derive(InitSpace)]
pub struct KeeperPolicy {
    /// Minimum bond (micro-USDC) to register and to be assigned.
    pub min_bond: u64,
    /// Seconds after a session's hard deadline by which the assigned keeper
    /// must have refunded every escrow.
    pub sla_seconds: i64,
    /// Paid from the keeper's bond to whoever refunds an escrow the keeper
    /// left open past its SLA (per escrow, capped by what the bond holds).
    pub escalation_reward: u64,
    pub bump: u8,
}

impl KeeperPolicy {
    /// PDA seed: `["keeper_policy"]` (one per program, like the config).
    pub const SEED: &'static [u8] = b"keeper_policy";

    pub fn check(min_bond: u64, sla_seconds: i64, escalation_reward: u64) -> Result<()> {
        require!(
            (1..=MAX_KEEPER_SLA_SECONDS).contains(&sla_seconds)
                && escalation_reward > 0
                && escalation_reward <= min_bond,
            RailsError::KeeperPolicyOutOfBounds
        );
        Ok(())
    }
}

/// A bonded keeper. Its bond sits in `["keeper_vault", keeper]`, a USDC
/// account owned by this PDA, never by the keeper's wallet.
#[account]
#[derive(InitSpace)]
pub struct Keeper {
    pub wallet: Pubkey,
    pub vault: Pubkey,
    /// Live session assignments; the bond can't leave while > 0.
    pub assignments: u32,
    /// Escrows third parties had to refund past this keeper's SLA.
    pub missed: u32,
    pub bump: u8,
}

impl Keeper {
    /// PDA seed prefix: `["keeper", wallet]`.
    pub const SEED: &'static [u8] = b"keeper";
    /// Vault seed prefix: `["keeper_vault", keeper]`.
    pub const VAULT_SEED: &'static [u8] = b"keeper_vault";
}

/// One keeper's duty for one session: every escrow refunded by `due_at`.
#[account]
#[derive(InitSpace)]
pub struct SessionKeeper {
    pub session: Pubkey,
    /// The `Keeper` PDA.
    pub keeper: Pubkey,
    /// `session.settle_deadline + policy.sla_seconds` at assignment.
    pub due_at: i64,
    /// `policy.escalation_reward` at assignment.
    pub reward: u64,
    /// Escrows escalated so far.
    pub escalations: u32,
    pub bump: u8,
}

impl SessionKeeper {
    /// PDA seed prefix: `["session_keeper", session]`.
    pub const SEED: &'static [u8] = b"session_keeper";
}

/// Idempotency + audit record for one coupon period.
#[account]
#[derive(InitSpace)]
pub struct CouponReceipt {
    /// Distributor that paid it.
    pub distributor: Pubkey,
    /// Period id (seed).
    pub period_id: u64,
    /// The split actually paid.
    pub split: CouponSplit,
    /// Cluster time.
    pub paid_at: i64,
    /// PDA bump.
    pub bump: u8,
}

impl CouponReceipt {
    /// PDA seed prefix: `["coupon", distributor, period_id_le]`.
    pub const SEED: &'static [u8] = b"coupon";
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
            registrar: Pubkey::new_from_array([8; 32]),
            fee_bps: 10,
            reserve_bps: 5,
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
                registrar: Pubkey::new_from_array([9; 32]),
                ..cfg()
            },
            RailsConfig {
                fee_bps: 11,
                ..cfg()
            },
            RailsConfig {
                reserve_bps: 6,
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
            fee_receiver: Pubkey::new_from_array([3; 32]),
            asset_mint: Pubkey::new_from_array([6; 32]),
            asset_decimals: 6,
            fee_bps: 10,
            reserve_bps: 5,
            rofr_authority: Pubkey::default(),
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
            total_asset: 0,
            fills_settled: 0,
            volume_settled: 0,
            gross_settled: 0,
            total_cash_shortfall: 0,
            total_asset_shortfall: 0,
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

    #[test]
    fn settled_session_releases_everything_but_unrevealed_bonds() {
        let s = session(SessionStatus::Settled, 0);
        // Settling only happens after reveals close; refunds open at once.
        assert_eq!(s.refund_route(true, 250).unwrap(), BondRoute::ToOwner);
        assert_eq!(s.refund_route(false, 250).unwrap(), BondRoute::ToReserve);
    }

    #[test]
    fn fill_split_exact_values() {
        // 1.5 NPCS (6dp) at 98.123456 USDC → 147.185184 USDC.
        let f = FillSplit::compute(1_500_000, 98_123_456, 6, 0, 5).unwrap();
        assert_eq!(f.gross, 147_185_184);
        assert_eq!(f.fee, 0); // AE-3: no per-trade fee (#147)
        assert_eq!(f.to_reserve, 73_592); // floor(147185184 × 5 / 10⁴)
        assert_eq!(f.to_seller, 147_185_184 - 73_592);
        // Any fee at all is refused.
        assert!(FillSplit::compute(1_500_000, 98_123_456, 6, 1, 5).is_err());
    }

    #[test]
    fn buyer_pays_the_ceiling() {
        // 1 base unit at 1 micro-USDC per whole unit = 1e-6 µUSDC → 1.
        let f = FillSplit::compute(1, 1, 6, 0, 0).unwrap();
        assert_eq!(f.gross, 1);
        assert_eq!(f.to_seller, 1);
        // Exact multiples don't round up.
        assert_eq!(FillSplit::compute(2_000_000, 7, 6, 0, 0).unwrap().gross, 14);
    }

    #[test]
    fn split_conserves_across_a_sweep() {
        let qtys = [
            1u64,
            7,
            999_999,
            1_000_000,
            123_456_789,
            u64::MAX / 1_000_000,
        ];
        let prices = [1u64, 3, 999_999, 1_000_000, 98_765_432];
        for &qty in &qtys {
            for &price in &prices {
                for (fee, res) in [(0, 0), (0, 1), (0, 5), (MAX_FEE_BPS, MAX_RESERVE_BPS)] {
                    for dec in [0u8, 6, 9] {
                        let Ok(f) = FillSplit::compute(qty, price, dec, fee, res) else {
                            continue; // only overflow may fail; checked below
                        };
                        assert_eq!(
                            u128::from(f.to_seller) + u128::from(f.fee) + u128::from(f.to_reserve),
                            u128::from(f.gross),
                            "qty={qty} price={price} dec={dec} fee={fee} res={res}"
                        );
                        let exact = u128::from(qty) * u128::from(price);
                        let unit = 10u128.pow(u32::from(dec));
                        assert!(u128::from(f.gross) * unit >= exact, "never under-pays");
                        assert!(
                            u128::from(f.gross) * unit < exact + unit,
                            "at most 1 µUSDC over"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn split_rejects_bad_inputs() {
        let e = |r: Result<FillSplit>| r.unwrap_err();
        assert_eq!(
            e(FillSplit::compute(0, 1, 6, 0, 0)),
            RailsError::ZeroAmount.into()
        );
        assert_eq!(
            e(FillSplit::compute(1, 0, 6, 0, 0)),
            RailsError::ZeroAmount.into()
        );
        assert_eq!(
            e(FillSplit::compute(1, 1, 6, MAX_FEE_BPS + 1, 0)),
            RailsError::FeeTooHigh.into()
        );
        assert_eq!(
            e(FillSplit::compute(1, 1, 6, 0, MAX_RESERVE_BPS + 1)),
            RailsError::FeeTooHigh.into()
        );
        // Gross that doesn't fit a token amount.
        assert_eq!(
            e(FillSplit::compute(u64::MAX, u64::MAX, 0, 0, 0)),
            RailsError::MathOverflow.into()
        );
    }

    // ── R3 ──────────────────────────────────────────────────────────────

    fn dest() -> PayoutDestination {
        PayoutDestination {
            owner: Pubkey::new_from_array([1; 32]),
            token_account: Pubkey::new_from_array([2; 32]),
            attestation_ref_hash: [3; 32],
            registrar: Pubkey::new_from_array([4; 32]),
            expires_at: 1_000,
            active: true,
            bump: 255,
        }
    }

    #[test]
    fn payout_destination_typed_reasons() {
        let good = Pubkey::new_from_array([2; 32]);
        assert!(dest().ensure_payable(&good, 999).is_ok());
        let e = |d: PayoutDestination, t: &Pubkey, now| d.ensure_payable(t, now).unwrap_err();
        assert_eq!(
            e(dest(), &Pubkey::new_from_array([9; 32]), 0),
            RailsError::DestinationNotAllowlisted.into()
        );
        assert_eq!(
            e(dest(), &good, 1_000),
            RailsError::DestinationExpired.into()
        );
        let revoked = PayoutDestination {
            active: false,
            ..dest()
        };
        // Revocation wins over everything else (block-list precedence).
        assert_eq!(e(revoked, &good, 0), RailsError::DestinationRevoked.into());
    }

    fn adv(principal: u64) -> Advance {
        Advance {
            facility: Pubkey::new_from_array([1; 32]),
            borrower: Pubkey::new_from_array([2; 32]),
            collateral_ref_hash: [0; 32],
            collateral_value: 0,
            advance_rate_bps: 0,
            principal,
            interest: 0,
            last_accrual: 0,
            bump: 255,
        }
    }

    #[test]
    fn max_advance_enforces_the_h1_ceiling() {
        // 50% of 1 000 USDC; floor against the borrower.
        assert_eq!(
            Advance::max_advance(1_000_000_000, 5_000).unwrap(),
            500_000_000
        );
        assert_eq!(Advance::max_advance(3, 5_000).unwrap(), 1);
        assert_eq!(
            Advance::max_advance(1_000_000_000, 2_500).unwrap(),
            250_000_000
        );
        assert_eq!(Advance::max_advance(u64::MAX, 5_000).unwrap(), u64::MAX / 2);
        assert_eq!(Advance::max_advance(1_000, 0).unwrap(), 0);
        for over in [5_001, 10_000, u64::MAX] {
            assert_eq!(
                Advance::max_advance(1_000_000_000, over).unwrap_err(),
                error!(RailsError::AdvanceRateTooHigh)
            );
        }
    }

    #[test]
    fn advance_accrues_pilot_rate_exactly() {
        // credit-math reference: 12% on 1,000 USDC for a year = 120 USDC.
        let mut a = adv(1_000_000_000);
        a.accrue(1_200, 31_536_000).unwrap();
        assert_eq!(a.interest, 120_000_000);
        assert_eq!(a.owed(), 1_120_000_000);
        // Clock going backwards accrues nothing and never rewinds.
        a.accrue(1_200, 10).unwrap();
        assert_eq!((a.interest, a.last_accrual), (120_000_000, 31_536_000));
    }

    #[test]
    fn repayment_is_interest_first_and_never_overcollects() {
        let mut a = adv(1_000);
        a.interest = 30;
        let r = a.apply(20);
        assert_eq!((r.interest, r.principal), (20, 0));
        let r = a.apply(500);
        assert_eq!((r.interest, r.principal), (10, 490));
        let r = a.apply(10_000); // more than owed
        assert_eq!((r.interest, r.principal, r.total()), (0, 510, 510));
        assert_eq!(a.owed(), 0);
    }

    fn coupon() -> CouponParams {
        CouponParams {
            fees_due: 10,
            senior_coupon_due: 100,
            reserve_target: 50,
            reserve_current: 20,
            senior_amort_due: 40,
            junior_coupon_due: 60,
        }
    }

    #[test]
    fn coupon_split_exact_lines() {
        // 1000: fees 10, senior 100+40, reserve 30, junior 60 + 760 residual.
        let c = CouponSplit::compute(1_000, &coupon()).unwrap();
        assert_eq!((c.fees, c.reserve, c.senior, c.junior), (10, 30, 140, 820));
        // 50: fees 10, senior coupon 40 (60 short, deferred), nothing else.
        let c = CouponSplit::compute(50, &coupon()).unwrap();
        assert_eq!((c.fees, c.senior, c.reserve, c.junior), (10, 40, 0, 0));
        assert_eq!(c.unpaid_senior_coupon, 60);
    }

    #[test]
    fn coupon_split_sums_exactly_to_cash_in() {
        for cash in [
            0u64,
            1,
            9,
            10,
            109,
            110,
            139,
            140,
            179,
            180,
            239,
            240,
            1_000_003,
            u64::MAX,
        ] {
            let c = CouponSplit::compute(cash, &coupon()).unwrap();
            assert_eq!(
                u128::from(c.fees)
                    + u128::from(c.reserve)
                    + u128::from(c.senior)
                    + u128::from(c.junior),
                u128::from(cash),
                "cash={cash}"
            );
        }
    }
}
