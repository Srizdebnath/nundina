//! AE-P8 — the gate's re-check inside every settlement (#133, #134).
//!
//! payment-rails' `settle_fill` / `settle_rofr_fill` CPI here before any
//! value moves, signed by its settlement PDA ([`SETTLEMENT_AUTHORITY`]).
//! If anything below fails, the fill fails with it: nothing is paid, nothing
//! is delivered, and the receipt is not written.
//!
//! **Order** (the first failure is the reason; CG-P5 precedence):
//! 1. block-list: seller, then buyer. A block beats every allow-side fact;
//! 2. market checklist (fill path only, #202, CG-4/CG-6): the issuer flags
//!    (`issuer_reporting_current`, `bad_actor_screened`) are set, and the
//!    fill is at least `min_ticket`;
//! 3. buyer admission (fill path only): the buyer holds a [`HolderLink`]
//!    from `validate`, and its SAS identity and accreditation attestations
//!    still pass [`evaluate`] now (open, signed by a market attester, in
//!    date for the settlement horizon, inside policy and exemption class)
//!    and still name the same `investor_id`;
//! 4. self-trade by investor (fill path, #202): the seller's own
//!    [`HolderLink`], when passed, names another `investor_id`;
//! 5. affiliate volume (fill path, L6, #202): with a limit on, a registered
//!    affiliate seller stays inside its window's volume;
//! 6. seasoning (C2, Rule-144 analog): the seller's seasoned quantity
//!    covers the fill, and it is debited ([`HolderLots::debit`]); in a
//!    Reg S market every lot drawn is past its distribution-compliance
//!    period (CG-3, L27);
//! 7. the buyer's new lot is appended, seasoning from now (fill path only).
//!
//! **Remaining accounts of `settle_check`** (#202; appended, optional, so
//! a caller built against the v1 account list keeps working). A slot
//! holding this program's id is "not passed":
//! - `[0]` the seller's [`HolderLink`] `["holder", mint, seller]` (may be
//!   empty). Passed, the self-trade check compares investors, not just
//!   wallets. Not passed, only wallets are compared (v1 behaviour).
//! - `[1]` the seller's [`AffiliateRecord`] `["affiliate", mint, seller]`,
//!   writable (may be empty). **Required** while the market's
//!   `affiliate_volume_limit` is on: without it the fill fails closed
//!   ([`GateError::AffiliateRecordRequired`]), so a caller cannot dodge the
//!   limit by leaving it out.
//!
//! **Today's caller passes neither.** payment-rails' `gate::check` builds
//! the `settle_check` account list itself and forwards no remaining
//! accounts, so on every fill through the rails both slots read as "not
//! passed": the self-trade check compares wallets (investor-level wash
//! trades are excluded earlier, at auction-engine's AE-P6 screen), and an
//! affiliate limit, if governance turns one on, fails every resale closed
//! until the rails forward the record. Wiring that is a rails change
//! (behind the Squads timelock); until then keep `affiliate_volume_limit`
//! at 0.
//!
//! **Exemption basis** returned for the transfer record (doc 07 L3–L5):
//! an accredited buyer → §4(a)(7); an admitted buyer the policy lets in
//! without accreditation → §4(a)(1½); the issuer's ROFR purchase → the
//! issue's own exemption. Codes are payment-rails' `ExemptionPath` indices.

use anchor_lang::prelude::*;

use crate::blocklist::{screen, BlockEntry};
use crate::compliance::{AffiliateRecord, Compliance};
use crate::eligibility::{evaluate, load, reject_error, store, HolderLink, MarketPolicy};
use crate::lots::{HolderLots, LotDraw, LotEvent, LotSource, FREE_LOT_ID};
use crate::{GateError, GateReject, MarketConfig};

/// The payment-rails program (GdWm…Pocn), the only caller of `settle_check*`.
pub const PAYMENT_RAILS_ID: Pubkey = pubkey!("GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn");
/// Seed of payment-rails' settlement PDA.
pub const SETTLEMENT_AUTHORITY_SEED: &[u8] = b"gate_settle";
/// `find_program_address(["gate_settle"], PAYMENT_RAILS_ID)`. A unit test
/// re-derives it.
pub const SETTLEMENT_AUTHORITY: Pubkey = pubkey!("GX6PXFskZdcA8L9FMgS81sAVtQNki4a2B5XFW9oWaqJm");

/// Exemption-path codes, the same indices as payment-rails' `ExemptionPath`
/// (and what a [`crate::Lot`] records in `exemption_path`). Append only.
pub mod exemption {
    /// Securities Act §4(a)(7) resale to an accredited buyer (doc 07 L4).
    pub const SECTION_4A7: u8 = 1;
    /// §4(a)(1½) resale (doc 07 L5).
    pub const SECTION_4A1_HALF: u8 = 2;
    /// Inside the exemption the issue was sold under (doc 07 L3).
    pub const ISSUANCE: u8 = 3;
}

/// What a passed check hands back to payment-rails (return data).
#[derive(Debug, Clone, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub struct SettleOutcome {
    /// The seller's lots the fill drew from, at most [`crate::MAX_DRAWS`].
    pub draws: Vec<LotDraw>,
    /// [`exemption`] code the transfer relies on.
    pub exemption_path: u8,
    /// The buyer's new lot; `None` on the ROFR path.
    pub buyer_lot_id: Option<u32>,
}

/// Emitted once per passed settle-time check (CG-P7).
#[event]
#[derive(Debug)]
pub struct SettleChecked {
    pub mint: Pubkey,
    pub seller: Pubkey,
    /// The buyer, or the issuer on the ROFR path.
    pub buyer: Pubkey,
    pub qty: u64,
    pub draws: Vec<LotDraw>,
    pub exemption_path: u8,
    pub buyer_lot_id: Option<u32>,
    pub rofr: bool,
    /// Seller's FREE bucket after the debit.
    pub seller_free_qty: u64,
}

fn require_unblocked(config: &MarketConfig, entry: &AccountInfo) -> Result<()> {
    match screen(config, entry)? {
        Some(r) => Err(reject_error(r)),
        None => Ok(()),
    }
}

fn finish(
    mint: Pubkey,
    seller: Pubkey,
    buyer: Pubkey,
    qty: u64,
    seller_lots: &HolderLots,
    outcome: SettleOutcome,
    rofr: bool,
) -> Result<SettleOutcome> {
    emit!(SettleChecked {
        mint,
        seller,
        buyer,
        qty,
        draws: outcome.draws.clone(),
        exemption_path: outcome.exemption_path,
        buyer_lot_id: outcome.buyer_lot_id,
        rofr,
        seller_free_qty: seller_lots.free_qty,
    });
    Ok(outcome)
}

/// Remaining account `i`, or `None` if it was not passed (absent, or this
/// program's id as the placeholder). When passed it must be `expected`.
fn optional_slot<'a, 'info>(
    remaining: &'a [AccountInfo<'info>],
    i: usize,
    expected: &Pubkey,
) -> Result<Option<&'a AccountInfo<'info>>> {
    match remaining.get(i) {
        None => Ok(None),
        Some(ai) if *ai.key == crate::ID => Ok(None),
        Some(ai) => {
            require_keys_eq!(*ai.key, *expected, GateError::RemainingAccountMismatch);
            Ok(Some(ai))
        }
    }
}

/// CG-3 / L27: every lot a sale drew from is past the Reg S
/// distribution-compliance period. FREE draws carry no date; they are
/// covered by `seasoning_seconds ≥ distribution_compliance_seconds`
/// (`MarketConfig::check_bounds`): FREE only ever holds lots merged once
/// seasoned.
pub fn require_distribution_compliance(
    config: &Compliance,
    draws: &[LotDraw],
    now: i64,
) -> Result<()> {
    for d in draws.iter().filter(|d| d.lot_id != FREE_LOT_ID) {
        require!(
            config.distribution_compliance_met(d.acquired_ts, now),
            GateError::DistributionCompliance
        );
    }
    Ok(())
}

/// L6: count a sale of `qty` against the seller's affiliate window. `rec`
/// is the seller's [`AffiliateRecord`] slot (`None` = not passed);
/// `holds_affiliate_lots` whether the seller's ledger carries a lot
/// flagged `affiliate`. With the limit off this changes nothing.
fn charge_affiliate(
    config: &Compliance,
    rec: Option<&AccountInfo>,
    holds_affiliate_lots: bool,
    qty: u64,
    now: i64,
) -> Result<()> {
    if config.affiliate_volume_limit == 0 {
        return Ok(());
    }
    // Fail closed: the record slot is what makes the limit unskippable.
    let ai = rec.ok_or(GateError::AffiliateRecordRequired)?;
    let Some(mut r) = load::<AffiliateRecord>(ai)? else {
        // Not a registered affiliate. Lots flagged `affiliate` (append_lot)
        // say otherwise: the authority must register the seller first.
        require!(!holds_affiliate_lots, GateError::AffiliateRecordRequired);
        return Ok(());
    };
    require!(ai.is_writable, GateError::AffiliateRecordRequired);
    r.charge(
        qty,
        now,
        config.affiliate_volume_limit,
        config.affiliate_window_seconds,
    )?;
    store(ai, &r)
}

/// `settle_check` body (see the instruction doc in `lib.rs`).
pub fn process_settle_check(
    ctx: Context<SettleCheck>,
    seller: Pubkey,
    buyer: Pubkey,
    qty: u64,
) -> Result<SettleOutcome> {
    require_keys_neq!(seller, buyer, GateError::SelfTrade);
    let a = ctx.accounts;
    let mint = a.config.mint;
    let now = Clock::get()?.unix_timestamp;
    require_unblocked(&a.config, &a.seller_block)?;
    require_unblocked(&a.config, &a.buyer_block)?;
    let compliance = Compliance::load(&a.config.to_account_info())?;
    compliance.check_resale(Some(qty))?;
    let seller_link = optional_slot(
        ctx.remaining_accounts,
        0,
        &Pubkey::find_program_address(
            &[HolderLink::SEED, mint.as_ref(), seller.as_ref()],
            &crate::ID,
        )
        .0,
    )?;
    let affiliate = optional_slot(
        ctx.remaining_accounts,
        1,
        &AffiliateRecord::address(&mint, &seller),
    )?;
    let eligible = match evaluate(
        &a.config,
        &a.policy,
        &buyer,
        &a.buyer_block,
        &a.identity_attestation,
        &a.accreditation_attestation,
        now,
    )? {
        Err(r) => return Err(reject_error(r)),
        Ok(e) if e.identity.investor_id != a.buyer_link.investor_id => {
            return Err(reject_error(GateReject::InvestorIdChanged))
        }
        Ok(e) => e,
    };
    // CG-3 (#202): the exemption class narrows the policy at settle too.
    if let Some(r) = compliance.class_reject(&eligible.identity, &eligible.accreditation) {
        return Err(reject_error(r));
    }
    // Two wallets of one investor are one party (#202): no wash fill.
    if let Some(link) = seller_link.map(load::<HolderLink>).transpose()?.flatten() {
        require!(
            link.investor_id != a.buyer_link.investor_id,
            GateError::SelfTrade
        );
    }
    let holds_affiliate_lots = a.seller_lots.live().iter().any(|l| l.affiliate);
    charge_affiliate(&compliance, affiliate, holds_affiliate_lots, qty, now)?;
    let draws = a.seller_lots.debit(qty, now)?;
    require_distribution_compliance(&compliance, &draws, now)?;
    let exemption_path = if eligible.accreditation.accredited {
        exemption::SECTION_4A7
    } else {
        exemption::SECTION_4A1_HALF
    };
    let lot = a.buyer_lots.append(
        qty,
        now,
        a.config.seasoning_seconds,
        LotSource::AuctionSettle,
        false,
        exemption_path,
    )?;
    emit!(LotEvent {
        kind: 0,
        mint,
        owner: buyer,
        lot_id: lot.lot_id,
        qty,
        restriction_expiry: lot.restriction_expiry,
        free_qty: a.buyer_lots.free_qty,
    });
    let outcome = SettleOutcome {
        draws,
        exemption_path,
        buyer_lot_id: Some(lot.lot_id),
    };
    finish(mint, seller, buyer, qty, &a.seller_lots, outcome, false)
}

/// `settle_check_rofr` body.
pub fn process_settle_check_rofr(
    ctx: Context<SettleCheckRofr>,
    seller: Pubkey,
    issuer: Pubkey,
    qty: u64,
) -> Result<SettleOutcome> {
    require_keys_neq!(seller, issuer, GateError::SelfTrade);
    let a = ctx.accounts;
    let now = Clock::get()?.unix_timestamp;
    require_unblocked(&a.config, &a.seller_block)?;
    require_unblocked(&a.config, &a.issuer_block)?;
    let draws = a.seller_lots.debit(qty, now)?;
    let outcome = SettleOutcome {
        draws,
        exemption_path: exemption::ISSUANCE,
        buyer_lot_id: None,
    };
    finish(
        a.config.mint,
        seller,
        issuer,
        qty,
        &a.seller_lots,
        outcome,
        true,
    )
}

#[derive(Accounts)]
#[instruction(seller: Pubkey, buyer: Pubkey)]
pub struct SettleCheck<'info> {
    /// payment-rails' settlement PDA: only `settle_fill` can sign for it.
    #[account(address = SETTLEMENT_AUTHORITY @ GateError::NotSettlementAuthority)]
    pub settlement_authority: Signer<'info>,
    #[account(seeds = [MarketConfig::SEED, mint.key().as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, MarketConfig>>,
    #[account(seeds = [MarketPolicy::SEED, mint.key().as_ref()], bump = policy.bump)]
    pub policy: Box<Account<'info, MarketPolicy>>,
    /// CHECK: the seller's block entry, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), seller.as_ref()], bump)]
    pub seller_block: UncheckedAccount<'info>,
    /// CHECK: the buyer's block entry, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), buyer.as_ref()], bump)]
    pub buyer_block: UncheckedAccount<'info>,
    /// The seller's ledger; a seller without one has nothing seasoned.
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), seller.as_ref()],
        bump = seller_lots.bump
    )]
    pub seller_lots: Box<Account<'info, HolderLots>>,
    /// The buyer's ledger (`validate` opened it).
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), buyer.as_ref()],
        bump = buyer_lots.bump
    )]
    pub buyer_lots: Box<Account<'info, HolderLots>>,
    /// The buyer's admission: must exist (CG-P1/P4).
    #[account(
        seeds = [HolderLink::SEED, mint.key().as_ref(), buyer.as_ref()],
        bump = buyer_link.bump
    )]
    pub buyer_link: Box<Account<'info, HolderLink>>,
    /// CHECK: the buyer's SAS identity attestation; verified in `evaluate`.
    pub identity_attestation: UncheckedAccount<'info>,
    /// CHECK: the buyer's SAS accreditation attestation; verified in `evaluate`.
    pub accreditation_attestation: UncheckedAccount<'info>,
    /// CHECK: the gated mint; derives every PDA above.
    pub mint: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(seller: Pubkey, issuer: Pubkey)]
pub struct SettleCheckRofr<'info> {
    #[account(address = SETTLEMENT_AUTHORITY @ GateError::NotSettlementAuthority)]
    pub settlement_authority: Signer<'info>,
    #[account(seeds = [MarketConfig::SEED, mint.key().as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, MarketConfig>>,
    /// CHECK: the seller's block entry, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), seller.as_ref()], bump)]
    pub seller_block: UncheckedAccount<'info>,
    /// CHECK: the issuer's block entry, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), issuer.as_ref()], bump)]
    pub issuer_block: UncheckedAccount<'info>,
    #[account(
        mut,
        seeds = [HolderLots::SEED, mint.key().as_ref(), seller.as_ref()],
        bump = seller_lots.bump
    )]
    pub seller_lots: Box<Account<'info, HolderLots>>,
    /// CHECK: the gated mint; derives every PDA above.
    pub mint: UncheckedAccount<'info>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settlement_authority_is_the_rails_pda() {
        let (pda, _) =
            Pubkey::find_program_address(&[SETTLEMENT_AUTHORITY_SEED], &PAYMENT_RAILS_ID);
        assert_eq!(pda, SETTLEMENT_AUTHORITY);
    }
}
