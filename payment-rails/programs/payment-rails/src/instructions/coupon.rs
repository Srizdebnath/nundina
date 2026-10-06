//! Tranche coupon distribution (R3; TE-P4): one period's cash, down the
//! `credit-math` waterfall, to the fee receiver, the reserve and the senior
//! and junior holders' **registered** payout destinations. The lines sum
//! exactly to the cash in, or nothing moves. One receipt per period.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::error::RailsError;
use crate::events::{PayoutKind, PayoutMade};
use crate::instructions::payout::load_destination;
use crate::instructions::token;
use crate::state::{
    require_operator_role, CouponParams, CouponReceipt, CouponSplit, EngineGrant, RailsConfig,
};

/// Distribute `cash` for `period_id`. The distributor (tranche-engine
/// signer = config operator) pays from its own USDC account.
pub fn process_distribute_coupon(
    ctx: Context<DistributeCoupon>,
    period_id: u64,
    cash: u64,
    params: CouponParams,
    period_ref_hash: [u8; 32],
) -> Result<()> {
    require_operator_role(
        &ctx.accounts.config,
        &ctx.accounts.operator.key(),
        ctx.remaining_accounts,
        EngineGrant::ROLE_COUPON,
    )?;
    require!(cash > 0, RailsError::ZeroAmount);
    let now = Clock::get()?.unix_timestamp;
    let split = CouponSplit::compute(cash, &params)?;

    // Holder lines must be allowlisted (checked before any cash moves).
    if split.senior > 0 {
        load_destination(&ctx.accounts.senior_destination, None)?
            .ensure_payable(&ctx.accounts.senior_token.key(), now)?;
    }
    if split.junior > 0 {
        load_destination(&ctx.accounts.junior_destination, None)?
            .ensure_payable(&ctx.accounts.junior_token.key(), now)?;
    }

    let source = ctx.accounts.source.to_account_info();
    let authority = ctx.accounts.operator.to_account_info();
    let legs = [
        (ctx.accounts.fee_receiver.to_account_info(), split.fees),
        (ctx.accounts.reserve.to_account_info(), split.reserve),
        (ctx.accounts.senior_token.to_account_info(), split.senior),
        (ctx.accounts.junior_token.to_account_info(), split.junior),
    ];
    for (to, amount) in legs {
        let dest_key = to.key();
        token::transfer(
            &ctx.accounts.token_program,
            &ctx.accounts.usdc_mint,
            source.clone(),
            to,
            authority.clone(),
            &[],
            amount,
        )?;
        if amount > 0 {
            emit!(PayoutMade {
                kind: PayoutKind::Coupon,
                source: source.key(),
                destination: dest_key,
                amount,
                reference_hash: period_ref_hash,
            });
        }
    }

    ctx.accounts.receipt.set_inner(CouponReceipt {
        distributor: ctx.accounts.operator.key(),
        period_id,
        split,
        paid_at: now,
        bump: ctx.bumps.receipt,
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(period_id: u64)]
pub struct DistributeCoupon<'info> {
    #[account(
        seeds = [RailsConfig::SEED],
        bump = config.bump,
        has_one = fee_receiver @ RailsError::FeeReceiverMismatch,
        has_one = reserve @ RailsError::ReserveMismatch,
        has_one = usdc_mint @ RailsError::MintMismatch
    )]
    pub config: Box<Account<'info, RailsConfig>>,
    /// Idempotency: one distribution per (distributor, period).
    #[account(
        init,
        payer = operator,
        space = 8 + CouponReceipt::INIT_SPACE,
        seeds = [CouponReceipt::SEED, operator.key().as_ref(), &period_id.to_le_bytes()],
        bump
    )]
    pub receipt: Box<Account<'info, CouponReceipt>>,
    /// Period cash (token program enforces the operator's authority).
    #[account(mut, constraint = source.mint == usdc_mint.key() @ RailsError::MintMismatch)]
    pub source: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub fee_receiver: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub reserve: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: resolved by `load_destination` (typed rejection).
    pub senior_destination: UncheckedAccount<'info>,
    #[account(mut)]
    pub senior_token: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: resolved by `load_destination` (typed rejection).
    pub junior_destination: UncheckedAccount<'info>,
    #[account(mut)]
    pub junior_token: Box<InterfaceAccount<'info, TokenAccount>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    /// Tranche-engine signer: the config operator, or an engine granted
    /// `ROLE_COUPON` (first remaining account, #136); pays the receipt rent.
    #[account(mut)]
    pub operator: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}
