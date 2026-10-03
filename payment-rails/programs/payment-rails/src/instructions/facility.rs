//! Liquidity-facility cash leg (R3; LF-P1/P2/P6, QC-P4).
//!
//! The rails hold the facility's USDC and move it; underwriting (the mark
//! itself vs `MARK_PROTECTIVE`, the maintenance ratio, health checks, the
//! liquidation ladder) belongs to the liquidity-facility engine, which
//! signs here as the curator and is not built. What the rails guarantee:
//! - every advance is bounded by the H1 hard ceiling (#94): the curator
//!   states the collateral mark and rate, the rate may not exceed
//!   `credit_math::MAX_ADVANCE_RATE_BPS` (50%), and the amount may not
//!   exceed mark × rate. The rails cannot verify the mark (no mark engine);
//!   they record it on the `Advance` so the bound is auditable;
//! - advances go only to the borrower's registered payout destination;
//! - repayment is interest-first, accrued with `credit-math`, never above
//!   what is owed;
//! - claim-sale proceeds repay the facility **before** the holder sees a
//!   micro-USDC (QC-P4 sweep order), and the two legs sum to the proceeds.
//!
//! Not here, and not anywhere else yet (#124, roadmap): a health check, a
//! liquidate instruction and the LF-P5 ladder (issuer ROFR → backstop
//! bidder → forced-sale auction → workout). With no mark engine there is no
//! mark state to read, so H4 / LF-P4 ("never liquidate on a stale mark") has
//! no implementation and no test. AC-9 is graded roadmap in SRS 08 §1.1.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::error::RailsError;
use crate::events::{FacilityAction, FacilityEvent, PayoutKind, PayoutMade};
use crate::instructions::payout::load_destination;
use crate::instructions::token;
use crate::state::{Advance, Facility, RailsConfig, Repayment, MAX_RATE_BPS};

fn payout_event(
    kind: PayoutKind,
    source: Pubkey,
    destination: Pubkey,
    amount: u64,
    ref_hash: [u8; 32],
) {
    emit!(PayoutMade {
        kind,
        source,
        destination,
        amount,
        reference_hash: ref_hash,
    });
}

/// Accrue, apply up to `amount` (interest first), and update the facility's
/// running totals. Returns what was applied.
fn apply_repayment(
    facility: &mut Facility,
    advance: &mut Advance,
    amount: u64,
    now: i64,
) -> Result<Repayment> {
    advance.accrue(facility.rate_bps, now)?;
    require!(advance.owed() > 0, RailsError::NothingOwed);
    let r = advance.apply(amount);
    facility.principal_outstanding = facility
        .principal_outstanding
        .checked_sub(r.principal)
        .ok_or(RailsError::MathUnderflow)?;
    facility.interest_collected = facility
        .interest_collected
        .checked_add(r.interest)
        .ok_or(RailsError::MathOverflow)?;
    Ok(r)
}

/// LF-P2: open a facility fund with its own vault. Operator-only (the
/// liquidity-facility engine's signer), which becomes its curator.
pub fn process_open_facility(
    ctx: Context<OpenFacility>,
    facility_id: u64,
    rate_bps: u64,
) -> Result<()> {
    require!(rate_bps <= MAX_RATE_BPS, RailsError::RateTooHigh);
    let curator = ctx.accounts.operator.key();
    let facility_key = ctx.accounts.facility.key();
    ctx.accounts.facility.set_inner(Facility {
        curator,
        facility_id,
        usdc_mint: ctx.accounts.usdc_mint.key(),
        vault: ctx.accounts.vault.key(),
        rate_bps,
        principal_outstanding: 0,
        total_funded: 0,
        interest_collected: 0,
        bump: ctx.bumps.facility,
    });
    emit!(FacilityEvent {
        action: FacilityAction::Opened,
        facility: facility_key,
        amount: 0,
        actor: curator,
    });
    Ok(())
}

/// LF-P2: add cash to the facility (treasury / lender). Anyone may fund.
pub fn process_fund_facility(ctx: Context<FundFacility>, amount: u64) -> Result<()> {
    require!(amount > 0, RailsError::ZeroAmount);
    token::transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        ctx.accounts.funder_token.to_account_info(),
        ctx.accounts.vault.to_account_info(),
        ctx.accounts.funder.to_account_info(),
        &[],
        amount,
    )?;
    let facility = &mut ctx.accounts.facility;
    facility.total_funded = facility
        .total_funded
        .checked_add(amount)
        .ok_or(RailsError::MathOverflow)?;
    emit!(FacilityEvent {
        action: FacilityAction::Funded,
        facility: facility.key(),
        amount,
        actor: ctx.accounts.funder.key(),
    });
    Ok(())
}

/// LF-P1: pay an advance of `amount` to `borrower`'s **registered** payout
/// destination. One live advance per (facility, borrower). Blocked while
/// paused or depegged (no new lending in a broken settlement asset).
/// `amount ≤ collateral_value × advance_rate_bps / 10⁴`, with the rate at
/// most the H1 ceiling (#94).
pub fn process_advance(
    ctx: Context<AdvanceFunds>,
    amount: u64,
    collateral_ref_hash: [u8; 32],
    collateral_value: u64,
    advance_rate_bps: u64,
) -> Result<()> {
    ctx.accounts.config.ensure_accepting()?;
    require!(amount > 0, RailsError::ZeroAmount);
    require!(
        amount <= Advance::max_advance(collateral_value, advance_rate_bps)?,
        RailsError::AdvanceExceedsRate
    );
    let now = Clock::get()?.unix_timestamp;
    let borrower = ctx.accounts.borrower.key();
    let dest = load_destination(&ctx.accounts.destination, Some(&borrower))?;
    dest.ensure_payable(&ctx.accounts.destination_token.key(), now)?;
    require!(
        ctx.accounts.vault.amount >= amount,
        RailsError::FacilityUnderfunded
    );

    let facility = &ctx.accounts.facility;
    let id_bytes = facility.facility_id.to_le_bytes();
    let seeds: &[&[u8]] = &[
        Facility::SEED,
        facility.curator.as_ref(),
        &id_bytes,
        &[facility.bump],
    ];
    token::transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        ctx.accounts.vault.to_account_info(),
        ctx.accounts.destination_token.to_account_info(),
        facility.to_account_info(),
        &[seeds],
        amount,
    )?;

    let facility_key = facility.key();
    ctx.accounts.advance.set_inner(Advance {
        facility: facility_key,
        borrower,
        collateral_ref_hash,
        collateral_value,
        advance_rate_bps,
        principal: amount,
        interest: 0,
        last_accrual: now,
        bump: ctx.bumps.advance,
    });
    let facility = &mut ctx.accounts.facility;
    facility.principal_outstanding = facility
        .principal_outstanding
        .checked_add(amount)
        .ok_or(RailsError::MathOverflow)?;
    payout_event(
        PayoutKind::FacilityAdvance,
        ctx.accounts.vault.key(),
        ctx.accounts.destination_token.key(),
        amount,
        collateral_ref_hash,
    );
    Ok(())
}

/// Close a fully repaid advance, rent back to the curator who funded it.
fn close_if_repaid<'info>(
    advance: &Account<'info, Advance>,
    facility: Pubkey,
    curator: AccountInfo<'info>,
    actor: Pubkey,
) -> Result<()> {
    if advance.owed() == 0 {
        advance.close(curator)?;
        emit!(FacilityEvent {
            action: FacilityAction::AdvanceClosed,
            facility,
            amount: 0,
            actor,
        });
    }
    Ok(())
}

/// LF-P6: repay up to `amount` (interest first). Anyone may pay — the
/// borrower, or a crank on its behalf. Never collects more than owed.
pub fn process_repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
    require!(amount > 0, RailsError::ZeroAmount);
    let now = Clock::get()?.unix_timestamp;
    let r = apply_repayment(
        &mut ctx.accounts.facility,
        &mut ctx.accounts.advance,
        amount,
        now,
    )?;
    token::transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        ctx.accounts.payer_token.to_account_info(),
        ctx.accounts.vault.to_account_info(),
        ctx.accounts.payer.to_account_info(),
        &[],
        r.total(),
    )?;
    payout_event(
        PayoutKind::FacilityRepayment,
        ctx.accounts.payer_token.key(),
        ctx.accounts.vault.key(),
        r.total(),
        ctx.accounts.advance.collateral_ref_hash,
    );
    close_if_repaid(
        &ctx.accounts.advance,
        ctx.accounts.facility.key(),
        ctx.accounts.curator.to_account_info(),
        ctx.accounts.payer.key(),
    )
}

/// QC-P4: route `amount` of claim-sale proceeds — **facility first**
/// (accrued interest, then principal), the remainder to the holder's
/// registered destination. `to_facility + to_holder == amount`, exactly.
/// Signed by whoever holds the proceeds (queue-claim engine / crank).
pub fn process_sweep_proceeds(ctx: Context<SweepProceeds>, amount: u64) -> Result<()> {
    require!(amount > 0, RailsError::ZeroAmount);
    let now = Clock::get()?.unix_timestamp;
    let r = apply_repayment(
        &mut ctx.accounts.facility,
        &mut ctx.accounts.advance,
        amount,
        now,
    )?;
    let to_facility = r.total();
    let to_holder = amount - to_facility; // apply() never exceeds `amount`
    let ref_hash = ctx.accounts.advance.collateral_ref_hash;

    // Holder leg must be allowlisted (checked before any cash moves).
    if to_holder > 0 {
        let holder = ctx.accounts.advance.borrower;
        let dest = load_destination(&ctx.accounts.holder_destination, Some(&holder))?;
        dest.ensure_payable(&ctx.accounts.holder_token.key(), now)?;
    }
    let source = ctx.accounts.proceeds_token.to_account_info();
    let authority = ctx.accounts.proceeds_authority.to_account_info();
    token::transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        source.clone(),
        ctx.accounts.vault.to_account_info(),
        authority.clone(),
        &[],
        to_facility,
    )?;
    token::transfer(
        &ctx.accounts.token_program,
        &ctx.accounts.usdc_mint,
        source,
        ctx.accounts.holder_token.to_account_info(),
        authority,
        &[],
        to_holder,
    )?;
    let src = ctx.accounts.proceeds_token.key();
    if to_facility > 0 {
        payout_event(
            PayoutKind::FacilityRepayment,
            src,
            ctx.accounts.vault.key(),
            to_facility,
            ref_hash,
        );
    }
    if to_holder > 0 {
        payout_event(
            PayoutKind::ClaimProceeds,
            src,
            ctx.accounts.holder_token.key(),
            to_holder,
            ref_hash,
        );
    }
    close_if_repaid(
        &ctx.accounts.advance,
        ctx.accounts.facility.key(),
        ctx.accounts.curator.to_account_info(),
        ctx.accounts.proceeds_authority.key(),
    )
}

#[derive(Accounts)]
#[instruction(facility_id: u64)]
pub struct OpenFacility<'info> {
    #[account(
        seeds = [RailsConfig::SEED],
        bump = config.bump,
        has_one = operator @ RailsError::NotOperator,
        has_one = usdc_mint @ RailsError::MintMismatch
    )]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        init,
        payer = operator,
        space = 8 + Facility::INIT_SPACE,
        seeds = [Facility::SEED, operator.key().as_ref(), &facility_id.to_le_bytes()],
        bump
    )]
    pub facility: Box<Account<'info, Facility>>,
    #[account(
        init,
        payer = operator,
        seeds = [Facility::VAULT_SEED, facility.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = facility,
        token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    /// Config operator (facility-engine signer); becomes the curator.
    #[account(mut)]
    pub operator: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct FundFacility<'info> {
    #[account(
        mut,
        seeds = [Facility::SEED, facility.curator.as_ref(), &facility.facility_id.to_le_bytes()],
        bump = facility.bump,
        has_one = vault,
        has_one = usdc_mint @ RailsError::MintMismatch
    )]
    pub facility: Box<Account<'info, Facility>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut, constraint = funder_token.mint == usdc_mint.key() @ RailsError::MintMismatch)]
    pub funder_token: Box<InterfaceAccount<'info, TokenAccount>>,
    pub funder: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct AdvanceFunds<'info> {
    /// Halt switches.
    #[account(seeds = [RailsConfig::SEED], bump = config.bump)]
    pub config: Box<Account<'info, RailsConfig>>,
    #[account(
        mut,
        seeds = [Facility::SEED, facility.curator.as_ref(), &facility.facility_id.to_le_bytes()],
        bump = facility.bump,
        has_one = curator @ RailsError::NotCurator,
        has_one = vault,
        has_one = usdc_mint @ RailsError::MintMismatch
    )]
    pub facility: Box<Account<'info, Facility>>,
    #[account(
        init,
        payer = curator,
        space = 8 + Advance::INIT_SPACE,
        seeds = [Advance::SEED, facility.key().as_ref(), borrower.key().as_ref()],
        bump
    )]
    pub advance: Box<Account<'info, Advance>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the borrower wallet; its registry entry is checked in the handler.
    pub borrower: UncheckedAccount<'info>,
    /// CHECK: resolved by `load_destination` so a non-allowlisted account
    /// fails with a typed reason instead of a framework error.
    pub destination: UncheckedAccount<'info>,
    /// Must equal the destination's registered token account.
    #[account(mut)]
    pub destination_token: Box<InterfaceAccount<'info, TokenAccount>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub curator: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Repay<'info> {
    #[account(
        mut,
        seeds = [Facility::SEED, facility.curator.as_ref(), &facility.facility_id.to_le_bytes()],
        bump = facility.bump,
        has_one = vault,
        has_one = curator,
        has_one = usdc_mint @ RailsError::MintMismatch
    )]
    pub facility: Box<Account<'info, Facility>>,
    #[account(
        mut,
        seeds = [Advance::SEED, facility.key().as_ref(), advance.borrower.as_ref()],
        bump = advance.bump,
        has_one = facility
    )]
    pub advance: Box<Account<'info, Advance>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: bound to `facility.curator`; receives the advance rent on close.
    #[account(mut)]
    pub curator: UncheckedAccount<'info>,
    #[account(mut, constraint = payer_token.mint == usdc_mint.key() @ RailsError::MintMismatch)]
    pub payer_token: Box<InterfaceAccount<'info, TokenAccount>>,
    pub payer: Signer<'info>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct SweepProceeds<'info> {
    #[account(
        mut,
        seeds = [Facility::SEED, facility.curator.as_ref(), &facility.facility_id.to_le_bytes()],
        bump = facility.bump,
        has_one = vault,
        has_one = curator,
        has_one = usdc_mint @ RailsError::MintMismatch
    )]
    pub facility: Box<Account<'info, Facility>>,
    /// The pledged claim's advance (QC-P4: claim pledged to this facility).
    #[account(
        mut,
        seeds = [Advance::SEED, facility.key().as_ref(), advance.borrower.as_ref()],
        bump = advance.bump,
        has_one = facility
    )]
    pub advance: Box<Account<'info, Advance>>,
    #[account(mut)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: bound to `facility.curator`; receives the advance rent on close.
    #[account(mut)]
    pub curator: UncheckedAccount<'info>,
    /// Where the sale proceeds sit (queue-claim engine / crank account).
    #[account(mut, constraint = proceeds_token.mint == usdc_mint.key() @ RailsError::MintMismatch)]
    pub proceeds_token: Box<InterfaceAccount<'info, TokenAccount>>,
    pub proceeds_authority: Signer<'info>,
    /// CHECK: resolved by `load_destination` (typed rejection).
    pub holder_destination: UncheckedAccount<'info>,
    /// Must equal the holder's registered token account.
    #[account(mut)]
    pub holder_token: Box<InterfaceAccount<'info, TokenAccount>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
}
