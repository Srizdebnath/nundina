//! `tranche-engine` — SPV-configured senior/junior tranching over NPCS.
//!
//! SRS 08 §4.6 (TE-P1..P7) · doc 06 §6 · #140.
//!
//! - TE-P1 `create_pool` refuses a pool without a `legal_doc_hash` (the
//!   pilot indenture, versioned in the repo) and a `waterfall_version`;
//!   nothing can be minted from a pool that does not have them.
//! - TE-P2 `deposit` takes NPCS collateral, values it at mark-engine's
//!   `MARK_PROTECTIVE`, and mints senior and junior Token-2022 tranches at
//!   the pool's attachment point. Junior accounts are frozen (lock-up);
//!   a junior exit goes through `exit_junior`, checked by TE-P5.
//! - TE-P3 three books: `unrealised_mark` (mark moves, `mark`; principal
//!   untouched), `realised_loss` (`realise_loss` writes principal down
//!   junior → reserve → senior via `credit_math::apply_loss`) and
//!   `recovery_receivable` (`record_recovery` restores in reverse order:
//!   senior first, then junior).
//! - TE-P4 `distribute`: one period's actual cash down the
//!   `credit_math::distribute_cash` waterfall — fees → senior coupon →
//!   reserve top-up → senior amortization → junior coupon → junior
//!   residual — moved by payment-rails `distribute_coupon` (signer PDA
//!   granted COUPON), every line evented there; this program books the
//!   reserve and the amortization.
//! - TE-P5 subordination invariant `junior_nav / total ≥ attachment_min`,
//!   checked before a junior exit (rejected) and after marks and losses
//!   (pool → Impaired).
//! - TE-P6 states Warehousing → Active → Standstill / Amortizing →
//!   Impaired → WoundDown; checked u128 arithmetic throughout.
//! - TE-P7 `human_override`: authority only, evented with the hash of the
//!   off-chain authority document.

#![deny(unsafe_code)]
#![allow(missing_docs)]
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::InstructionData;
use anchor_spl::token_interface::{
    self, Burn, FreezeAccount, Mint, MintTo, ThawAccount, TokenAccount, TokenInterface,
    TransferChecked,
};
use credit_math::{accrue_interest, apply_loss, distribute_cash, mul_div, WaterfallParams};
use mark_engine::Feed;

declare_id!("CvcaeyuPu7SrFJ91WniJfBhUbQBkb8QQVL9fyK4wcFAS");

pub const SIGNER_SEED: &[u8] = b"te_signer";
pub const POOL_SEED: &[u8] = b"pool";
pub const SENIOR_SEED: &[u8] = b"senior";
pub const JUNIOR_SEED: &[u8] = b"junior";
pub const COLLATERAL_SEED: &[u8] = b"collateral";
pub const CASH_SEED: &[u8] = b"cash";
/// H2 interim floor on the attachment point (junior share). doc 06 §11
/// has no plan-of-record value yet (#123); a pool cannot go below this.
pub const MIN_ATTACHMENT_BPS: u64 = 500;
pub const MAX_ATTACHMENT_BPS: u64 = 9_000;
/// Ceiling on any coupon / fee rate (APR, bps).
pub const MAX_RATE_BPS: u64 = 5_000;

const BPS: u64 = 10_000;

pub fn signer_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SIGNER_SEED], &crate::ID)
}

/// Rails coupon receipts are keyed by (distributor, period): pool and
/// period together make the id unique across pools.
pub fn rails_period_id(pool_id: u32, period: u32) -> u64 {
    (u64::from(pool_id) << 32) | u64::from(period)
}

#[program]
pub mod tranche_engine {
    use super::*;

    /// TE-P1: a pool exists only with its indenture hash and waterfall
    /// version. The signer is the SPV authority (multisig).
    pub fn create_pool(ctx: Context<CreatePool>, pool_id: u32, params: PoolParams) -> Result<()> {
        params.validate()?;
        let p = &mut ctx.accounts.pool;
        p.pool_id = pool_id;
        p.authority = ctx.accounts.authority.key();
        p.params = params;
        p.collateral_mint = ctx.accounts.collateral_mint.key();
        p.feed = ctx.accounts.feed.key();
        p.senior_mint = ctx.accounts.senior_mint.key();
        p.junior_mint = ctx.accounts.junior_mint.key();
        p.collateral_vault = ctx.accounts.collateral_vault.key();
        p.cash = ctx.accounts.cash.key();
        p.state = PoolState::Warehousing;
        p.bump = ctx.bumps.pool;
        p.signer_bump = ctx.bumps.signer;
        emit!(PoolCreated {
            pool: p.key(),
            legal_doc_hash: params.legal_doc_hash,
            waterfall_version: params.waterfall_version,
            attachment_bps: params.attachment_bps,
        });
        Ok(())
    }

    /// TE-P2: deposit collateral at `MARK_PROTECTIVE` (Fresh mark only);
    /// mint junior = value × attachment, senior = the rest. Junior is
    /// frozen (lock-up).
    pub fn deposit(ctx: Context<Deposit>, qty: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(qty > 0, TeError::ZeroAmount);
        let p = &ctx.accounts.pool;
        require!(
            matches!(p.state, PoolState::Warehousing | PoolState::Active),
            TeError::WrongState
        );
        let feed = &ctx.accounts.feed;
        require!(feed.allows_new_advance(now), TeError::MarkNotFresh);
        let value = value_of(feed, qty, now)?;
        let junior = u64::try_from(
            credit_math::checked_ceil_div(
                u128::from(value) * u128::from(p.params.attachment_bps),
                u128::from(BPS),
            )
            .map_err(|_| TeError::MathOverflow)?,
        )
        .map_err(|_| TeError::MathOverflow)?;
        let senior = value - junior;
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.collateral_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.depositor_collateral.to_account_info(),
                    mint: ctx.accounts.collateral_mint.to_account_info(),
                    to: ctx.accounts.collateral_vault.to_account_info(),
                    authority: ctx.accounts.depositor.to_account_info(),
                },
            ),
            qty,
            ctx.accounts.collateral_mint.decimals,
        )?;
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[p.signer_bump]];
        let tp = ctx.accounts.tranche_token_program.key();
        for (mint, to, amount) in [
            (
                &ctx.accounts.senior_mint,
                &ctx.accounts.depositor_senior,
                senior,
            ),
            (
                &ctx.accounts.junior_mint,
                &ctx.accounts.depositor_junior,
                junior,
            ),
        ] {
            if to.is_frozen() {
                token_interface::thaw_account(CpiContext::new_with_signer(
                    tp,
                    ThawAccount {
                        account: to.to_account_info(),
                        mint: mint.to_account_info(),
                        authority: ctx.accounts.signer.to_account_info(),
                    },
                    &[seeds],
                ))?;
            }
            token_interface::mint_to(
                CpiContext::new_with_signer(
                    tp,
                    MintTo {
                        mint: mint.to_account_info(),
                        to: to.to_account_info(),
                        authority: ctx.accounts.signer.to_account_info(),
                    },
                    &[seeds],
                ),
                amount,
            )?;
        }
        // Junior lock-up: transfer-restricted until an exit.
        token_interface::freeze_account(CpiContext::new_with_signer(
            tp,
            FreezeAccount {
                account: ctx.accounts.depositor_junior.to_account_info(),
                mint: ctx.accounts.junior_mint.to_account_info(),
                authority: ctx.accounts.signer.to_account_info(),
            },
            &[seeds],
        ))?;
        let p = &mut ctx.accounts.pool;
        p.collateral_qty = p
            .collateral_qty
            .checked_add(qty)
            .ok_or(TeError::MathOverflow)?;
        p.senior_principal = p
            .senior_principal
            .checked_add(senior)
            .ok_or(TeError::MathOverflow)?;
        p.junior_principal = p
            .junior_principal
            .checked_add(junior)
            .ok_or(TeError::MathOverflow)?;
        emit!(Deposited {
            pool: p.key(),
            qty,
            value,
            senior,
            junior,
        });
        Ok(())
    }

    /// Warehousing → Active (authority). Coupons accrue from here.
    pub fn activate(ctx: Context<PoolAdmin>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let p = &mut ctx.accounts.pool;
        require!(p.state == PoolState::Warehousing, TeError::WrongState);
        require!(p.senior_principal > 0, TeError::ZeroAmount);
        p.last_distribution = now;
        set_state(p, PoolState::Active, [0; 32]);
        Ok(())
    }

    /// TE-P3 MARK book: collateral at `MARK_PROTECTIVE` against principal.
    /// Marks never touch principal; a mark that breaches the attachment
    /// floor impairs the pool (TE-P5). Permissionless.
    pub fn mark(ctx: Context<Mark>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let value = value_of(&ctx.accounts.feed, ctx.accounts.pool.collateral_qty, now)?;
        let p = &mut ctx.accounts.pool;
        let principal = i128::from(p.senior_principal) + i128::from(p.junior_principal);
        p.unrealised_mark =
            i64::try_from(i128::from(value) - principal).map_err(|_| TeError::MathOverflow)?;
        p.last_value = value;
        books(p);
        if !subordination_ok(p.senior_principal, value, p.params.attachment_bps)
            && matches!(p.state, PoolState::Active | PoolState::Amortizing)
        {
            set_state(p, PoolState::Impaired, [0; 32]);
        }
        Ok(())
    }

    /// TE-P3 REALISED book: write a realised loss down junior → reserve →
    /// senior (`credit_math::apply_loss`); the same amount becomes a
    /// recovery receivable. Authority only.
    pub fn realise_loss(ctx: Context<PoolAdmin>, amount: u64, ref_hash: [u8; 32]) -> Result<()> {
        require!(amount > 0, TeError::ZeroAmount);
        let p = &mut ctx.accounts.pool;
        let r = apply_loss(
            u128::from(amount),
            u128::from(p.junior_principal),
            u128::from(p.reserve_level),
            u128::from(p.senior_principal),
        )
        .map_err(|_| TeError::LossAboveCapital)?;
        let n = |v: u128| u64::try_from(v).map_err(|_| error!(TeError::MathOverflow));
        let (jh, rh, sh) = (n(r.junior_hit)?, n(r.reserve_hit)?, n(r.senior_hit)?);
        p.junior_principal -= jh;
        p.senior_principal -= sh;
        p.reserve_level = n(r.reserve_after)?;
        p.junior_loss += jh;
        p.senior_loss += sh;
        p.realised_loss += amount;
        p.recovery_receivable += amount;
        emit!(LossRealised {
            pool: p.key(),
            amount,
            junior_hit: jh,
            reserve_hit: rh,
            senior_hit: sh,
            ref_hash,
        });
        books(p);
        if p.senior_principal + p.junior_principal > 0
            && !subordination_ok(
                p.senior_principal,
                p.senior_principal + p.junior_principal,
                p.params.attachment_bps,
            )
            && matches!(p.state, PoolState::Active | PoolState::Amortizing)
        {
            set_state(p, PoolState::Impaired, ref_hash);
        }
        Ok(())
    }

    /// TE-P3 RECOVERY book: a recovery reverses losses in reverse-waterfall
    /// order — senior first, then junior. Authority only.
    pub fn record_recovery(ctx: Context<PoolAdmin>, amount: u64, ref_hash: [u8; 32]) -> Result<()> {
        let p = &mut ctx.accounts.pool;
        require!(
            amount > 0 && amount <= p.recovery_receivable,
            TeError::RecoveryAboveReceivable
        );
        let to_senior = amount.min(p.senior_loss);
        let to_junior = (amount - to_senior).min(p.junior_loss);
        p.senior_loss -= to_senior;
        p.senior_principal += to_senior;
        p.junior_loss -= to_junior;
        p.junior_principal += to_junior;
        p.recovery_receivable -= amount;
        emit!(RecoveryRecorded {
            pool: p.key(),
            amount,
            to_senior,
            to_junior,
            ref_hash,
        });
        books(p);
        Ok(())
    }

    /// TE-P4: one period's actual `cash` (paid in by the caller) down the
    /// waterfall through rails `distribute_coupon`. Coupons accrue per
    /// second since the last distribution; Amortizing pays senior
    /// principal. Remaining accounts: the rails `DistributeCoupon` list +
    /// the signer's `EngineGrant`.
    pub fn distribute<'info>(
        ctx: Context<'info, Distribute<'info>>,
        period: u32,
        cash: u64,
    ) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(cash > 0, TeError::ZeroAmount);
        let p = &ctx.accounts.pool;
        require!(
            matches!(p.state, PoolState::Active | PoolState::Amortizing),
            TeError::WrongState
        );
        require!(period == p.periods, TeError::WrongPeriod);
        let dt = u64::try_from(now - p.last_distribution).unwrap_or(0);
        let acc = |principal: u64, bps: u64| -> Result<u64> {
            let v = accrue_interest(u128::from(principal), bps, dt)
                .map_err(|_| TeError::MathOverflow)?;
            u64::try_from(v).map_err(|_| error!(TeError::MathOverflow))
        };
        let total = p.senior_principal + p.junior_principal;
        let params = payment_rails::CouponParams {
            fees_due: acc(total, p.params.fees_bps)?,
            senior_coupon_due: acc(p.senior_principal, p.params.senior_coupon_bps)?,
            reserve_target: p.params.reserve_target,
            reserve_current: p.reserve_level,
            senior_amort_due: if p.state == PoolState::Amortizing {
                p.senior_principal
            } else {
                0
            },
            junior_coupon_due: acc(p.junior_principal, p.params.junior_coupon_bps)?,
        };
        // The same waterfall the rails run (books the reserve + amort).
        let w = distribute_cash(
            u128::from(cash),
            &WaterfallParams {
                fees_due: u128::from(params.fees_due),
                senior_coupon_due: u128::from(params.senior_coupon_due),
                reserve_target: u128::from(params.reserve_target),
                reserve_current: u128::from(params.reserve_current),
                senior_amort_due: u128::from(params.senior_amort_due),
                junior_coupon_due: u128::from(params.junior_coupon_due),
            },
        )
        .map_err(|_| TeError::MathOverflow)?;

        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.payer_usdc.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.cash.to_account_info(),
                    authority: ctx.accounts.payer.to_account_info(),
                },
            ),
            cash,
            ctx.accounts.usdc_mint.decimals,
        )?;
        // The signer PDA pays the rails receipt rent: the payer funds it.
        let rent = Rent::get()?.minimum_balance(8 + payment_rails::CouponReceipt::INIT_SPACE);
        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ctx.accounts.signer.to_account_info(),
                },
            ),
            rent,
        )?;
        let signer = ctx.accounts.signer.key();
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 14, TeError::RailsAccounts);
        // DistributeCoupon: config0 receipt1 source2 fee3 reserve4
        // senior_dest5 senior_token6 junior_dest7 junior_token8 mint9
        // operator10 token_program11 system12 + grant.
        require_keys_eq!(rails[2].key(), p.cash, TeError::RailsAccounts);
        require_keys_eq!(rails[10].key(), signer, TeError::RailsAccounts);
        require_keys_eq!(
            rails[6].key(),
            p.params.senior_payee_token,
            TeError::RailsAccounts
        );
        require_keys_eq!(
            rails[8].key(),
            p.params.junior_payee_token,
            TeError::RailsAccounts
        );
        require_keys_eq!(
            ctx.accounts.rails_program.key(),
            payment_rails::ID,
            TeError::RailsAccounts
        );
        let metas = rails
            .iter()
            .map(|a| AccountMeta {
                pubkey: a.key(),
                is_signer: a.is_signer || a.key() == signer,
                is_writable: a.is_writable,
            })
            .collect();
        let mut infos = rails.to_vec();
        infos.push(ctx.accounts.rails_program.to_account_info());
        invoke_signed(
            &Instruction {
                program_id: payment_rails::ID,
                accounts: metas,
                data: payment_rails::instruction::DistributeCoupon {
                    period_id: rails_period_id(p.pool_id, period),
                    cash,
                    params,
                    period_ref_hash: p.params.legal_doc_hash,
                }
                .data(),
            },
            &infos,
            &[&[SIGNER_SEED, &[p.signer_bump]]],
        )?;
        let n = |v: u128| u64::try_from(v).map_err(|_| error!(TeError::MathOverflow));
        let p = &mut ctx.accounts.pool;
        p.reserve_level += n(w.paid_reserve)?;
        p.senior_principal -= n(w.paid_senior_amort)?;
        p.periods += 1;
        p.last_distribution = now;
        emit!(PeriodDistributed {
            pool: p.key(),
            period,
            cash,
            fees: n(w.paid_fees)?,
            senior_coupon: n(w.paid_senior_coupon)?,
            reserve: n(w.paid_reserve)?,
            senior_amort: n(w.paid_senior_amort)?,
            junior_coupon: n(w.paid_junior_coupon)?,
            junior_residual: n(w.junior_residual)?,
            unpaid_senior_coupon: n(w.unpaid_senior_coupon)?,
        });
        Ok(())
    }

    /// TE-P5: a junior holder exits `amount` junior units for the pro-rata
    /// collateral — only if the pool stays at or above its attachment
    /// floor afterwards (otherwise the exit must be an auction sale to a
    /// replacement holder).
    pub fn exit_junior(ctx: Context<ExitJunior>, amount: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        require!(amount > 0, TeError::ZeroAmount);
        let p = &ctx.accounts.pool;
        require!(
            matches!(p.state, PoolState::Active | PoolState::Amortizing),
            TeError::WrongState
        );
        let supply = ctx.accounts.junior_mint.supply;
        require!(amount <= supply, TeError::ZeroAmount);
        // Junior principal and collateral attributable to `amount`.
        let principal_out = u64::try_from(
            mul_div(
                u128::from(p.junior_principal),
                u128::from(amount),
                u128::from(supply),
            )
            .map_err(|_| TeError::MathOverflow)?,
        )
        .map_err(|_| TeError::MathOverflow)?;
        let value = value_of(&ctx.accounts.feed, p.collateral_qty, now)?;
        let qty_out = if value == 0 {
            0
        } else {
            u64::try_from(
                mul_div(
                    u128::from(p.collateral_qty),
                    u128::from(principal_out),
                    u128::from(value),
                )
                .map_err(|_| TeError::MathOverflow)?,
            )
            .map_err(|_| TeError::MathOverflow)?
        };
        // Pre-state-change check of the post-exit pool.
        let value_after = value.saturating_sub(principal_out);
        require!(
            subordination_ok(p.senior_principal, value_after, p.params.attachment_bps),
            TeError::SubordinationBreach
        );
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[p.signer_bump]];
        let tp = ctx.accounts.tranche_token_program.key();
        let holder_junior = &ctx.accounts.holder_junior;
        if holder_junior.is_frozen() {
            token_interface::thaw_account(CpiContext::new_with_signer(
                tp,
                ThawAccount {
                    account: holder_junior.to_account_info(),
                    mint: ctx.accounts.junior_mint.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ))?;
        }
        token_interface::burn(
            CpiContext::new(
                tp,
                Burn {
                    mint: ctx.accounts.junior_mint.to_account_info(),
                    from: holder_junior.to_account_info(),
                    authority: ctx.accounts.holder.to_account_info(),
                },
            ),
            amount,
        )?;
        token_interface::freeze_account(CpiContext::new_with_signer(
            tp,
            FreezeAccount {
                account: holder_junior.to_account_info(),
                mint: ctx.accounts.junior_mint.to_account_info(),
                authority: ctx.accounts.signer.to_account_info(),
            },
            &[seeds],
        ))?;
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.collateral_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.collateral_mint.to_account_info(),
                    to: ctx.accounts.holder_collateral.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ),
            qty_out,
            ctx.accounts.collateral_mint.decimals,
        )?;
        let p = &mut ctx.accounts.pool;
        p.junior_principal -= principal_out;
        p.collateral_qty -= qty_out;
        emit!(JuniorExited {
            pool: p.key(),
            amount,
            principal: principal_out,
            collateral: qty_out,
        });
        Ok(())
    }

    /// TE-P6: governed transitions (authority).
    pub fn set_pool_state(ctx: Context<PoolAdmin>, to: PoolState) -> Result<()> {
        let p = &mut ctx.accounts.pool;
        let ok = matches!(
            (p.state, to),
            (PoolState::Active, PoolState::Standstill)
                | (PoolState::Standstill, PoolState::Active)
                | (PoolState::Active, PoolState::Amortizing)
                | (PoolState::Standstill, PoolState::Amortizing)
                | (
                    PoolState::Amortizing | PoolState::Impaired,
                    PoolState::WoundDown
                )
        );
        require!(ok, TeError::WrongState);
        set_state(p, to, [0; 32]);
        Ok(())
    }

    /// TE-P7: human override on a contract/indenture conflict — any
    /// state, authority only, evented with the authority document hash.
    pub fn human_override(
        ctx: Context<PoolAdmin>,
        to: PoolState,
        authority_doc_hash: [u8; 32],
    ) -> Result<()> {
        require!(authority_doc_hash != [0; 32], TeError::MissingDoc);
        let p = &mut ctx.accounts.pool;
        emit!(HumanOverride {
            pool: p.key(),
            from: p.state,
            to,
            authority_doc_hash,
        });
        set_state(p, to, authority_doc_hash);
        Ok(())
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

/// Collateral value at `MARK_PROTECTIVE` (micro-USDC, floored).
pub fn value_of(feed: &Feed, qty: u64, now: i64) -> Result<u64> {
    let price = feed.pico_to_price(feed.mark_protective(now)?)?;
    let v = mul_div(
        u128::from(qty),
        u128::from(price),
        10u128.pow(u32::from(feed.asset_decimals)),
    )
    .map_err(|_| TeError::MathOverflow)?;
    u64::try_from(v).map_err(|_| error!(TeError::MathOverflow))
}

/// TE-P5: `junior_nav / total ≥ attachment`, with the senior protected
/// first: `junior_nav = total − min(senior, total)`.
pub fn subordination_ok(senior: u64, total: u64, attachment_bps: u64) -> bool {
    if total == 0 {
        return false;
    }
    let junior_nav = total - senior.min(total);
    u128::from(junior_nav) * u128::from(BPS) >= u128::from(total) * u128::from(attachment_bps)
}

fn set_state(p: &mut Pool, to: PoolState, doc: [u8; 32]) {
    emit!(PoolStateChanged {
        pool_id: p.pool_id,
        from: p.state,
        to,
        doc_hash: doc,
    });
    p.state = to;
}

fn books(p: &Pool) {
    emit!(Books {
        pool_id: p.pool_id,
        senior_principal: p.senior_principal,
        junior_principal: p.junior_principal,
        unrealised_mark: p.unrealised_mark,
        realised_loss: p.realised_loss,
        recovery_receivable: p.recovery_receivable,
        reserve_level: p.reserve_level,
    });
}

// ── state ───────────────────────────────────────────────────────────────

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum PoolState {
    Warehousing,
    Active,
    Standstill,
    Amortizing,
    Impaired,
    WoundDown,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct PoolParams {
    /// SHA-256 of the pilot indenture (TE-P1; required).
    pub legal_doc_hash: [u8; 32],
    /// Waterfall version of record (TE-P1; required).
    pub waterfall_version: u32,
    /// Junior share = attachment point, and the subordination floor.
    pub attachment_bps: u64,
    pub senior_coupon_bps: u64,
    pub junior_coupon_bps: u64,
    pub fees_bps: u64,
    /// Reserve account refill target (micro-USDC).
    pub reserve_target: u64,
    /// Registered rails payout accounts of the senior / junior holders
    /// (pilot: one holder of record per tranche).
    pub senior_payee_token: Pubkey,
    pub junior_payee_token: Pubkey,
}

impl PoolParams {
    pub fn validate(&self) -> Result<()> {
        require!(
            self.legal_doc_hash != [0; 32] && self.waterfall_version > 0,
            TeError::MissingDoc
        );
        require!(
            (MIN_ATTACHMENT_BPS..=MAX_ATTACHMENT_BPS).contains(&self.attachment_bps),
            TeError::AttachmentOutOfBounds
        );
        require!(
            self.senior_coupon_bps <= MAX_RATE_BPS
                && self.junior_coupon_bps <= MAX_RATE_BPS
                && self.fees_bps <= MAX_RATE_BPS,
            TeError::RateOutOfBounds
        );
        Ok(())
    }
}

#[account]
#[derive(InitSpace)]
pub struct Pool {
    pub pool_id: u32,
    pub authority: Pubkey,
    pub params: PoolParams,
    pub collateral_mint: Pubkey,
    pub feed: Pubkey,
    pub senior_mint: Pubkey,
    pub junior_mint: Pubkey,
    pub collateral_vault: Pubkey,
    pub cash: Pubkey,
    pub state: PoolState,
    pub collateral_qty: u64,
    pub senior_principal: u64,
    pub junior_principal: u64,
    /// MARK book: collateral value − principal (signed).
    pub unrealised_mark: i64,
    pub last_value: u64,
    /// REALISED book.
    pub realised_loss: u64,
    pub senior_loss: u64,
    pub junior_loss: u64,
    /// RECOVERY book.
    pub recovery_receivable: u64,
    /// Ledger of the reserve line (the cash sits in the rails reserve).
    pub reserve_level: u64,
    pub periods: u32,
    pub last_distribution: i64,
    pub bump: u8,
    pub signer_bump: u8,
}

// ── accounts ────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(pool_id: u32)]
pub struct CreatePool<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Pool::INIT_SPACE,
        seeds = [POOL_SEED, &pool_id.to_le_bytes()],
        bump
    )]
    pub pool: Box<Account<'info, Pool>>,
    #[account(constraint = feed.asset_mint == collateral_mint.key() @ TeError::FeedMismatch)]
    pub feed: Box<Account<'info, Feed>>,
    pub collateral_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init,
        payer = authority,
        seeds = [SENIOR_SEED, pool.key().as_ref()],
        bump,
        mint::decimals = 6,
        mint::authority = signer,
        mint::freeze_authority = signer,
        mint::token_program = tranche_token_program,
    )]
    pub senior_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init,
        payer = authority,
        seeds = [JUNIOR_SEED, pool.key().as_ref()],
        bump,
        mint::decimals = 6,
        mint::authority = signer,
        mint::freeze_authority = signer,
        mint::token_program = tranche_token_program,
    )]
    pub junior_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init,
        payer = authority,
        seeds = [COLLATERAL_SEED, pool.key().as_ref()],
        bump,
        token::mint = collateral_mint,
        token::authority = signer,
        token::token_program = collateral_token_program,
    )]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        init,
        payer = authority,
        seeds = [CASH_SEED, pool.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = signer,
        token::token_program = token_program,
    )]
    pub cash: Box<InterfaceAccount<'info, TokenAccount>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    /// CHECK: the signer PDA (mint/freeze/vault authority, rails operator).
    #[account(seeds = [SIGNER_SEED], bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub authority: Signer<'info>,
    /// This program, to locate its ProgramData. A pool's coupons run through the rails on this program's granted signer PDA; the SPV authority that creates it is the upgrade authority (multisig).
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, crate::program::TrancheEngine>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ TeError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,
    pub tranche_token_program: Interface<'info, TokenInterface>,
    pub collateral_token_program: Interface<'info, TokenInterface>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        has_one = feed @ TeError::FeedMismatch,
        has_one = senior_mint @ TeError::FeedMismatch,
        has_one = junior_mint @ TeError::FeedMismatch,
        has_one = collateral_vault @ TeError::FeedMismatch,
        has_one = collateral_mint @ TeError::FeedMismatch
    )]
    pub pool: Box<Account<'info, Pool>>,
    pub feed: Box<Account<'info, Feed>>,
    #[account(mut)]
    pub senior_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub junior_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub collateral_mint: Box<InterfaceAccount<'info, Mint>>,
    pub depositor: Signer<'info>,
    #[account(mut, token::authority = depositor)]
    pub depositor_collateral: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = senior_mint)]
    pub depositor_senior: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = junior_mint)]
    pub depositor_junior: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the signer PDA.
    #[account(seeds = [SIGNER_SEED], bump = pool.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    pub tranche_token_program: Interface<'info, TokenInterface>,
    pub collateral_token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct PoolAdmin<'info> {
    #[account(mut, has_one = authority @ TeError::NotAuthority)]
    pub pool: Box<Account<'info, Pool>>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct Mark<'info> {
    #[account(mut, has_one = feed @ TeError::FeedMismatch)]
    pub pool: Box<Account<'info, Pool>>,
    pub feed: Box<Account<'info, Feed>>,
}

#[derive(Accounts)]
pub struct Distribute<'info> {
    #[account(mut, has_one = cash @ TeError::RailsAccounts)]
    pub pool: Box<Account<'info, Pool>>,
    #[account(mut)]
    pub cash: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut)]
    pub payer_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    /// CHECK: the signer PDA (rails coupon operator).
    #[account(mut, seeds = [SIGNER_SEED], bump = pool.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    /// CHECK: payment-rails (address-checked in the handler).
    pub rails_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExitJunior<'info> {
    #[account(
        mut,
        has_one = feed @ TeError::FeedMismatch,
        has_one = junior_mint @ TeError::FeedMismatch,
        has_one = collateral_vault @ TeError::FeedMismatch,
        has_one = collateral_mint @ TeError::FeedMismatch
    )]
    pub pool: Box<Account<'info, Pool>>,
    pub feed: Box<Account<'info, Feed>>,
    #[account(mut)]
    pub junior_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    pub collateral_mint: Box<InterfaceAccount<'info, Mint>>,
    pub holder: Signer<'info>,
    #[account(mut, token::mint = junior_mint, token::authority = holder)]
    pub holder_junior: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = collateral_mint)]
    pub holder_collateral: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the signer PDA.
    #[account(seeds = [SIGNER_SEED], bump = pool.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    pub tranche_token_program: Interface<'info, TokenInterface>,
    pub collateral_token_program: Interface<'info, TokenInterface>,
}

// ── events ──────────────────────────────────────────────────────────────

#[event]
pub struct PoolCreated {
    pub pool: Pubkey,
    pub legal_doc_hash: [u8; 32],
    pub waterfall_version: u32,
    pub attachment_bps: u64,
}

#[event]
pub struct Deposited {
    pub pool: Pubkey,
    pub qty: u64,
    pub value: u64,
    pub senior: u64,
    pub junior: u64,
}

#[event]
pub struct Books {
    pub pool_id: u32,
    pub senior_principal: u64,
    pub junior_principal: u64,
    pub unrealised_mark: i64,
    pub realised_loss: u64,
    pub recovery_receivable: u64,
    pub reserve_level: u64,
}

#[event]
pub struct LossRealised {
    pub pool: Pubkey,
    pub amount: u64,
    pub junior_hit: u64,
    pub reserve_hit: u64,
    pub senior_hit: u64,
    pub ref_hash: [u8; 32],
}

#[event]
pub struct RecoveryRecorded {
    pub pool: Pubkey,
    pub amount: u64,
    pub to_senior: u64,
    pub to_junior: u64,
    pub ref_hash: [u8; 32],
}

/// TE-P4: every line, with its distribution character.
#[event]
pub struct PeriodDistributed {
    pub pool: Pubkey,
    pub period: u32,
    pub cash: u64,
    pub fees: u64,
    pub senior_coupon: u64,
    pub reserve: u64,
    pub senior_amort: u64,
    pub junior_coupon: u64,
    pub junior_residual: u64,
    pub unpaid_senior_coupon: u64,
}

#[event]
pub struct JuniorExited {
    pub pool: Pubkey,
    pub amount: u64,
    pub principal: u64,
    pub collateral: u64,
}

#[event]
pub struct PoolStateChanged {
    pub pool_id: u32,
    pub from: PoolState,
    pub to: PoolState,
    pub doc_hash: [u8; 32],
}

#[event]
pub struct HumanOverride {
    pub pool: Pubkey,
    pub from: PoolState,
    pub to: PoolState,
    pub authority_doc_hash: [u8; 32],
}

// ── errors (append-only) ────────────────────────────────────────────────

#[error_code]
pub enum TeError {
    #[msg("pool needs its indenture hash and waterfall version (TE-P1)")]
    MissingDoc,
    #[msg("attachment point outside the hard bounds")]
    AttachmentOutOfBounds,
    #[msg("coupon / fee rate above the ceiling")]
    RateOutOfBounds,
    #[msg("amount must be positive")]
    ZeroAmount,
    #[msg("pool is not in a state that allows this")]
    WrongState,
    #[msg("deposits need a Fresh mark")]
    MarkNotFresh,
    #[msg("feed / mint / vault does not match the pool")]
    FeedMismatch,
    #[msg("signer is not the pool authority")]
    NotAuthority,
    #[msg("loss exceeds junior + reserve + senior capital")]
    LossAboveCapital,
    #[msg("recovery exceeds the receivable")]
    RecoveryAboveReceivable,
    #[msg("period out of order")]
    WrongPeriod,
    #[msg("rails accounts do not match the pool")]
    RailsAccounts,
    #[msg("junior exit would breach the attachment floor (TE-P5)")]
    SubordinationBreach,
    #[msg("arithmetic overflow")]
    MathOverflow,
    #[msg("Only the program's upgrade authority may do this")]
    NotUpgradeAuthority,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subordination_protects_senior_first() {
        assert!(subordination_ok(80, 100, 2_000));
        assert!(!subordination_ok(81, 100, 2_000));
        // A 25% mark-down wipes the 20% junior: breached.
        assert!(!subordination_ok(80, 75, 2_000));
        assert!(!subordination_ok(0, 0, 2_000));
    }

    #[test]
    fn period_ids_are_unique_per_pool() {
        assert_ne!(rails_period_id(1, 0), rails_period_id(0, 1));
        assert_eq!(rails_period_id(1, 2), (1 << 32) | 2);
    }

    #[test]
    fn params_require_the_indenture() {
        let p = PoolParams {
            legal_doc_hash: [1; 32],
            waterfall_version: 1,
            attachment_bps: 2_000,
            senior_coupon_bps: 600,
            junior_coupon_bps: 1_200,
            fees_bps: 10,
            reserve_target: 0,
            senior_payee_token: Pubkey::new_unique(),
            junior_payee_token: Pubkey::new_unique(),
        };
        assert!(p.validate().is_ok());
        let mut q = p;
        q.legal_doc_hash = [0; 32];
        assert!(q.validate().is_err());
        let mut q = p;
        q.waterfall_version = 0;
        assert!(q.validate().is_err());
        let mut q = p;
        q.attachment_bps = MIN_ATTACHMENT_BPS - 1;
        assert!(q.validate().is_err());
    }
}
