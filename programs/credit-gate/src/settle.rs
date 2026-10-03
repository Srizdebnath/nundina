//! AE-P8 — the gate's re-check inside every settlement (#133, #134).
//!
//! payment-rails' `settle_fill` / `settle_rofr_fill` CPI here before any
//! value moves, signed by its settlement PDA ([`SETTLEMENT_AUTHORITY`]).
//! If anything below fails, the fill fails with it: nothing is paid, nothing
//! is delivered, and the receipt is not written.
//!
//! **Order** (the first failure is the reason; CG-P5 precedence):
//! 1. block-list: seller, then buyer. A block beats every allow-side fact;
//! 2. buyer admission (fill path only): the buyer holds a [`HolderLink`]
//!    from `validate`, and its SAS identity and accreditation attestations
//!    still pass [`evaluate`] now (open, signed by a market attester, in
//!    date for the settlement horizon, inside policy) and still name the
//!    same `investor_id`;
//! 3. seasoning (C2, Rule-144 analog): the seller's seasoned quantity
//!    covers the fill, and it is debited ([`HolderLots::debit`]);
//! 4. the buyer's new lot is appended, seasoning from now (fill path only).
//!
//! **Exemption basis** returned for the transfer record (doc 07 L3–L5):
//! an accredited buyer → §4(a)(7); an admitted buyer the policy lets in
//! without accreditation → §4(a)(1½); the issuer's ROFR purchase → the
//! issue's own exemption. Codes are payment-rails' `ExemptionPath` indices.

use anchor_lang::prelude::*;

use crate::blocklist::{screen, BlockEntry};
use crate::eligibility::{evaluate, reject_error, HolderLink, MarketPolicy};
use crate::lots::{HolderLots, LotDraw, LotEvent, LotSource};
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

/// `settle_check` body (see the instruction doc in `lib.rs`).
pub fn process_settle_check(
    ctx: Context<SettleCheck>,
    seller: Pubkey,
    buyer: Pubkey,
    qty: u64,
) -> Result<SettleOutcome> {
    require_keys_neq!(seller, buyer, GateError::SelfTrade);
    let a = ctx.accounts;
    let now = Clock::get()?.unix_timestamp;
    require_unblocked(&a.config, &a.seller_block)?;
    require_unblocked(&a.config, &a.buyer_block)?;
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
    let draws = a.seller_lots.debit(qty, now)?;
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
    let mint = a.config.mint;
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
