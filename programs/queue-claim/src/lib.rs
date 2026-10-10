//! `queue-claim` — a tokenized redemption-queue position.
//!
//! SRS 08 §4.4 (QC-P1..P5) · doc 06 §5 · #138. The headline primitive: a
//! pro-rated redemption request becomes a dated, priceable claim that can
//! be **sold in an auction session** or **pledged for a USDC advance**.
//!
//! - QC-P1 `request_redemption` escrows the holder's NPCS in a request
//!   vault; only the configured attester (the issuer / transfer-agent
//!   bridge) can `attest` it, with its queue rank and the expected fill
//!   schedule.
//! - QC-P2 attestation mints the **claim position token**: a Token-2022
//!   mint per request, supply 1, decimals 0, mint and freeze authority =
//!   this program's signer PDA. The holder's token account is frozen
//!   (non-transferable) until the issuer sets `transferable` or
//!   `pledgeable` (`set_flags`, issuer consent). States: Requested →
//!   Attested → PartiallyFilled → Retired, plus Cancelled / Frozen /
//!   Restructured.
//! - QC-P3 sell path: the claim token is an ordinary asset to
//!   `auction-engine` + `payment-rails` (its own market class and its own
//!   mark-engine feed). `quote_pv` computes the claim's model value with
//!   `credit_math::pv_of_claim` from the attested schedule and the
//!   underlying asset's `MARK_PROTECTIVE` — what the nav-publisher
//!   publishes into the claim's feed.
//! - QC-P4 pledge path: `pledge` hands the token to `liquidity-facility`
//!   for an advance; `record_fill` books each real window execution (cash
//!   in, NPCS out to the issuer); `sweep_pledged` sends a pledged claim's
//!   proceeds through rails `sweep_proceeds` — facility repayment first,
//!   then the holder. An unpledged claim's holder `collect`s.
//! - QC-P5 claims are a separate mint: they never touch the NPCS holder
//!   governor.

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
use credit_math::pv::{pv_of_claim, ExpectedFill};
use mark_engine::Feed;

declare_id!("2EpAnJkvw6FH8j97k7WRE72E7oyarZpf7t2TYa26red4");

pub const SIGNER_SEED: &[u8] = b"qc_signer";
pub const QUEUE_SEED: &[u8] = b"queue";
pub const REQUEST_SEED: &[u8] = b"request";
pub const NPCS_VAULT_SEED: &[u8] = b"npcs";
pub const CASH_VAULT_SEED: &[u8] = b"cash";
pub const CLAIM_MINT_SEED: &[u8] = b"claim_mint";
pub const CLAIM_ACCOUNT_SEED: &[u8] = b"claim_account";
/// Expected-fill schedule entries per request.
pub const MAX_SCHEDULE: usize = 8;

pub fn signer_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SIGNER_SEED], &crate::ID)
}

#[program]
pub mod queue_claim {
    use super::*;

    /// One queue per asset. The signer is the issuer authority (consent
    /// flags, fills, freezes); `attester` attests requests (QC-P1).
    pub fn init_queue(ctx: Context<InitQueue>, attester: Pubkey) -> Result<()> {
        let q = &mut ctx.accounts.queue;
        q.authority = ctx.accounts.authority.key();
        q.attester = attester;
        q.asset_mint = ctx.accounts.asset_mint.key();
        q.usdc_mint = ctx.accounts.usdc_mint.key();
        q.requests = 0;
        q.bump = ctx.bumps.queue;
        q.signer_bump = signer_pda().1;
        emit!(QueueConfigured {
            queue: q.key(),
            authority: q.authority,
            attester,
        });
        Ok(())
    }

    /// QC-P1: the holder submits a redemption request; `qty` NPCS move
    /// into the request vault until filled or cancelled.
    pub fn request_redemption(
        ctx: Context<RequestRedemption>,
        qty: u64,
        window_date: i64,
    ) -> Result<()> {
        require!(qty > 0, QcError::ZeroAmount);
        let now = Clock::get()?.unix_timestamp;
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.asset_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.holder_asset.to_account_info(),
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.npcs_vault.to_account_info(),
                    authority: ctx.accounts.holder.to_account_info(),
                },
            ),
            qty,
            ctx.accounts.asset_mint.decimals,
        )?;
        let queue = &mut ctx.accounts.queue;
        let id = queue.requests;
        queue.requests += 1;
        let r = &mut ctx.accounts.request;
        r.queue = queue.key();
        r.id = id;
        r.holder = ctx.accounts.holder.key();
        r.qty = qty;
        r.window_date = window_date;
        r.request_ts = now;
        r.npcs_vault = ctx.accounts.npcs_vault.key();
        r.state = ClaimState::Requested;
        r.bump = ctx.bumps.request;
        emit!(RequestSubmitted {
            request: r.key(),
            holder: r.holder,
            qty,
            window_date,
        });
        Ok(())
    }

    /// QC-P1/P2: the attester confirms the request (queue rank + expected
    /// fill schedule). Mints the claim position token to the holder and
    /// freezes it: non-transferable until issuer consent.
    pub fn attest(ctx: Context<Attest>, rank: u32, schedule: Vec<ScheduledFill>) -> Result<()> {
        require!(
            !schedule.is_empty() && schedule.len() <= MAX_SCHEDULE,
            QcError::BadSchedule
        );
        let r = &mut ctx.accounts.request;
        require!(r.state == ClaimState::Requested, QcError::WrongState);
        let total: u64 = schedule.iter().map(|s| s.qty).sum();
        require!(total <= r.qty, QcError::BadSchedule);
        let bump = ctx.accounts.queue.signer_bump;
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[bump]];
        let tp = ctx.accounts.claim_token_program.key();
        token_interface::mint_to(
            CpiContext::new_with_signer(
                tp,
                MintTo {
                    mint: ctx.accounts.claim_mint.to_account_info(),
                    to: ctx.accounts.holder_claim.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ),
            1,
        )?;
        token_interface::freeze_account(CpiContext::new_with_signer(
            tp,
            FreezeAccount {
                account: ctx.accounts.holder_claim.to_account_info(),
                mint: ctx.accounts.claim_mint.to_account_info(),
                authority: ctx.accounts.signer.to_account_info(),
            },
            &[seeds],
        ))?;
        r.rank = rank;
        r.schedule = schedule;
        r.claim_mint = ctx.accounts.claim_mint.key();
        r.holder_claim = ctx.accounts.holder_claim.key();
        r.cash_vault = ctx.accounts.cash_vault.key();
        r.state = ClaimState::Attested;
        emit!(RequestAttested {
            request: r.key(),
            rank,
            claim_mint: r.claim_mint,
        });
        Ok(())
    }

    /// QC-P2: issuer consent, set independently. Either flag thaws the
    /// original holder account (the token must be able to move into an
    /// auction escrow or the facility's custody); neither re-freezes it.
    pub fn set_flags(ctx: Context<SetFlags>, transferable: bool, pledgeable: bool) -> Result<()> {
        let r = &mut ctx.accounts.request;
        require!(
            !matches!(
                r.state,
                ClaimState::Requested | ClaimState::Cancelled | ClaimState::Retired
            ),
            QcError::WrongState
        );
        let bump = ctx.accounts.queue.signer_bump;
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[bump]];
        let open = transferable || pledgeable;
        let frozen = ctx.accounts.holder_claim.is_frozen();
        let tp = ctx.accounts.claim_token_program.key();
        if open && frozen {
            token_interface::thaw_account(CpiContext::new_with_signer(
                tp,
                ThawAccount {
                    account: ctx.accounts.holder_claim.to_account_info(),
                    mint: ctx.accounts.claim_mint.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ))?;
        } else if !open && !frozen {
            token_interface::freeze_account(CpiContext::new_with_signer(
                tp,
                FreezeAccount {
                    account: ctx.accounts.holder_claim.to_account_info(),
                    mint: ctx.accounts.claim_mint.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ))?;
        }
        r.transferable = transferable;
        r.pledgeable = pledgeable;
        emit!(FlagsSet {
            request: r.key(),
            transferable,
            pledgeable,
        });
        Ok(())
    }

    /// QC-P4: pledge the claim into a liquidity-facility for `amount` USDC.
    /// Remaining accounts: the facility `PledgeAndAdvance` list followed
    /// by its rails `AdvanceFunds` list.
    pub fn pledge<'info>(ctx: Context<'info, Pledge<'info>>, amount: u64) -> Result<()> {
        let r = &ctx.accounts.request;
        require!(r.pledgeable, QcError::NotPledgeable);
        require!(r.pledge.is_none(), QcError::AlreadyPledged);
        require!(
            matches!(r.state, ClaimState::Attested | ClaimState::PartiallyFilled),
            QcError::WrongState
        );
        let holder = ctx.accounts.holder.key();
        let accts = ctx.remaining_accounts;
        // 20 facility accounts (6 of them its LF-P5 gate custody group,
        // unused for an ungated claim mint) + 11 rails `AdvanceFunds`.
        require!(accts.len() == 31, QcError::FacilityAccounts);
        // PledgeAndAdvance: config0 … borrower6, borrower_asset7, asset_mint8.
        require_keys_eq!(accts[6].key(), holder, QcError::FacilityAccounts);
        require_keys_eq!(accts[8].key(), r.claim_mint, QcError::FacilityAccounts);
        let facility_cfg = liquidity_facility::FacilityConfig::try_deserialize(
            &mut &accts[0].try_borrow_data()?[..],
        )?;
        require_keys_eq!(
            *accts[0].owner,
            liquidity_facility::ID,
            QcError::FacilityAccounts
        );
        require_keys_eq!(
            ctx.accounts.facility_program.key(),
            liquidity_facility::ID,
            QcError::FacilityAccounts
        );
        let metas = accts
            .iter()
            .map(|a| AccountMeta {
                pubkey: a.key(),
                is_signer: a.is_signer,
                is_writable: a.is_writable,
            })
            .collect();
        let mut infos = accts.to_vec();
        infos.push(ctx.accounts.facility_program.to_account_info());
        anchor_lang::solana_program::program::invoke(
            &Instruction {
                program_id: liquidity_facility::ID,
                accounts: metas,
                data: liquidity_facility::instruction::PledgeAndAdvance { qty: 1, amount }.data(),
            },
            &infos,
        )?;
        let r = &mut ctx.accounts.request;
        r.pledge = Some(PledgeRef {
            facility_config: accts[0].key(),
            rails_facility: facility_cfg.rails_facility,
            borrower: holder,
        });
        emit!(ClaimPledged {
            request: r.key(),
            facility: facility_cfg.rails_facility,
            borrower: holder,
            amount,
        });
        Ok(())
    }

    /// QC-P4: a real window execution. The issuer pays `fill_qty × price`
    /// USDC into the claim's cash vault (rounded up: the issuer initiates)
    /// and receives the redeemed NPCS from the request vault.
    pub fn record_fill(ctx: Context<RecordFill>, fill_qty: u64, price: u64) -> Result<()> {
        let r = &ctx.accounts.request;
        require!(
            matches!(r.state, ClaimState::Attested | ClaimState::PartiallyFilled),
            QcError::WrongState
        );
        require!(fill_qty > 0 && price > 0, QcError::ZeroAmount);
        let left = r.qty - r.filled_qty;
        require!(fill_qty <= left, QcError::OverFill);
        let decimals = ctx.accounts.asset_mint.decimals;
        let cash = credit_math::checked_ceil_div(
            u128::from(fill_qty) * u128::from(price),
            10u128.pow(u32::from(decimals)),
        )
        .map_err(|_| QcError::MathOverflow)?;
        let cash = u64::try_from(cash).map_err(|_| QcError::MathOverflow)?;
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.issuer_usdc.to_account_info(),
                    mint: ctx.accounts.usdc_mint.to_account_info(),
                    to: ctx.accounts.cash_vault.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
            ),
            cash,
            ctx.accounts.usdc_mint.decimals,
        )?;
        let bump = ctx.accounts.queue.signer_bump;
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[bump]];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.asset_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.npcs_vault.to_account_info(),
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.issuer_asset.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ),
            fill_qty,
            decimals,
        )?;
        let r = &mut ctx.accounts.request;
        r.filled_qty += fill_qty;
        r.cash_received = r.cash_received.saturating_add(cash);
        r.state = if r.filled_qty == r.qty {
            ClaimState::Retired
        } else {
            ClaimState::PartiallyFilled
        };
        emit!(FillRecorded {
            request: r.key(),
            fill_qty,
            price,
            cash,
            filled_qty: r.filled_qty,
            state: r.state,
        });
        Ok(())
    }

    /// QC-P4: the holder of an unpledged claim token takes the proceeds.
    pub fn collect(ctx: Context<Collect>) -> Result<()> {
        let r = &ctx.accounts.request;
        require!(r.pledge.is_none(), QcError::Pledged);
        let amount = ctx.accounts.cash_vault.amount;
        require!(amount > 0, QcError::ZeroAmount);
        pay_out(
            &ctx.accounts.queue,
            &ctx.accounts.signer,
            &ctx.accounts.cash_vault,
            &ctx.accounts.holder_usdc,
            &ctx.accounts.usdc_mint,
            &ctx.accounts.token_program,
            amount,
        )?;
        emit!(ProceedsPaid {
            request: r.key(),
            to: ctx.accounts.holder_usdc.key(),
            amount,
        });
        Ok(())
    }

    /// QC-P4: a pledged claim's proceeds go through rails
    /// `sweep_proceeds`: facility repayment first, the rest to the
    /// borrower's registered destination. Permissionless. Remaining
    /// accounts: the rails `SweepProceeds` list.
    pub fn sweep_pledged<'info>(ctx: Context<'info, SweepPledged<'info>>) -> Result<()> {
        let r = &ctx.accounts.request;
        let pledge = r.pledge.ok_or(QcError::NotPledged)?;
        let amount = ctx.accounts.cash_vault.amount;
        require!(amount > 0, QcError::ZeroAmount);
        let signer = ctx.accounts.signer.key();
        let rails = ctx.remaining_accounts;
        require!(rails.len() == 10, QcError::FacilityAccounts);
        require_keys_eq!(
            rails[0].key(),
            pledge.rails_facility,
            QcError::FacilityAccounts
        );
        let advance = Pubkey::find_program_address(
            &[
                payment_rails::Advance::SEED,
                pledge.rails_facility.as_ref(),
                pledge.borrower.as_ref(),
            ],
            &payment_rails::ID,
        )
        .0;
        require_keys_eq!(rails[1].key(), advance, QcError::FacilityAccounts);
        require_keys_eq!(rails[4].key(), r.cash_vault, QcError::FacilityAccounts);
        require_keys_eq!(rails[5].key(), signer, QcError::FacilityAccounts);
        require_keys_eq!(
            ctx.accounts.rails_program.key(),
            payment_rails::ID,
            QcError::FacilityAccounts
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
        let bump = ctx.accounts.queue.signer_bump;
        invoke_signed(
            &Instruction {
                program_id: payment_rails::ID,
                accounts: metas,
                data: payment_rails::instruction::SweepProceeds { amount }.data(),
            },
            &infos,
            &[&[SIGNER_SEED, &[bump]]],
        )?;
        let r = &mut ctx.accounts.request;
        // The rails closed the advance at zero: the pledge is discharged.
        if rails[1].data_is_empty() {
            r.pledge = None;
        }
        emit!(PledgedProceedsSwept {
            request: r.key(),
            amount,
            discharged: r.pledge.is_none(),
        });
        Ok(())
    }

    /// Clear a pledge whose advance the rails have closed (repaid in full
    /// by any path). Permissionless.
    pub fn discharge(ctx: Context<Discharge>) -> Result<()> {
        let r = &mut ctx.accounts.request;
        let pledge = r.pledge.ok_or(QcError::NotPledged)?;
        let advance = Pubkey::find_program_address(
            &[
                payment_rails::Advance::SEED,
                pledge.rails_facility.as_ref(),
                pledge.borrower.as_ref(),
            ],
            &payment_rails::ID,
        )
        .0;
        require_keys_eq!(
            ctx.accounts.advance.key(),
            advance,
            QcError::FacilityAccounts
        );
        require!(ctx.accounts.advance.data_is_empty(), QcError::Pledged);
        r.pledge = None;
        emit!(PledgedProceedsSwept {
            request: r.key(),
            amount: 0,
            discharged: true,
        });
        Ok(())
    }

    /// QC-P3: the claim's model value — `credit_math::pv_of_claim` over the
    /// unfilled part of the attested schedule, each expected fill priced at
    /// the underlying asset's `MARK_PROTECTIVE`, discounted per period.
    /// Evented for the nav-publisher, which publishes it to the claim feed.
    pub fn quote_pv(ctx: Context<QuotePv>, discount_bps_per_period: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let r = &ctx.accounts.request;
        let feed = &ctx.accounts.asset_feed;
        let price = u128::from(feed.pico_to_price(feed.mark_protective(now)?)?);
        let mut filled = r.filled_qty;
        let fills: Vec<ExpectedFill> = r
            .schedule
            .iter()
            .map(|s| {
                let done = filled.min(s.qty);
                filled -= done;
                ExpectedFill {
                    period: s.period,
                    qty_base: u128::from(s.qty - done),
                    price_micros: price,
                }
            })
            .collect();
        let pv = pv_of_claim(&fills, discount_bps_per_period).map_err(|_| QcError::MathOverflow)?;
        let pv = u64::try_from(pv).map_err(|_| QcError::MathOverflow)?;
        emit!(ClaimPv {
            request: r.key(),
            pv,
            discount_bps_per_period,
            mark_price: price as u64,
            at: now,
        });
        Ok(())
    }

    /// Cancel before any fill (QC-P2 Cancelled): the holder returns the
    /// claim token (burned) and gets the NPCS back.
    pub fn cancel(ctx: Context<Cancel>) -> Result<()> {
        let r = &ctx.accounts.request;
        require!(
            matches!(r.state, ClaimState::Requested | ClaimState::Attested),
            QcError::WrongState
        );
        require!(r.pledge.is_none(), QcError::Pledged);
        let bump = ctx.accounts.queue.signer_bump;
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[bump]];
        if r.state == ClaimState::Attested {
            let claim = ctx
                .accounts
                .holder_claim
                .as_ref()
                .ok_or(QcError::ClaimNotHeld)?;
            let claim_mint = ctx
                .accounts
                .claim_mint
                .as_ref()
                .ok_or(QcError::ClaimNotHeld)?;
            let tp = ctx
                .accounts
                .claim_token_program
                .as_ref()
                .ok_or(QcError::ClaimNotHeld)?;
            require!(
                claim.owner == ctx.accounts.holder.key() && claim.amount == 1,
                QcError::ClaimNotHeld
            );
            if claim.is_frozen() {
                token_interface::thaw_account(CpiContext::new_with_signer(
                    tp.key(),
                    ThawAccount {
                        account: claim.to_account_info(),
                        mint: claim_mint.to_account_info(),
                        authority: ctx.accounts.signer.to_account_info(),
                    },
                    &[seeds],
                ))?;
            }
            token_interface::burn(
                CpiContext::new(
                    tp.key(),
                    Burn {
                        mint: claim_mint.to_account_info(),
                        from: claim.to_account_info(),
                        authority: ctx.accounts.holder.to_account_info(),
                    },
                ),
                1,
            )?;
        }
        let qty = r.qty;
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.asset_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.npcs_vault.to_account_info(),
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.holder_asset.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(),
                },
                &[seeds],
            ),
            qty,
            ctx.accounts.asset_mint.decimals,
        )?;
        let r = &mut ctx.accounts.request;
        r.state = ClaimState::Cancelled;
        emit!(StateChanged {
            request: r.key(),
            state: r.state,
        });
        Ok(())
    }

    /// QC-P2 Frozen (NAV suspended / fund gates fully) and back, or
    /// Restructured (redemptions suspended indefinitely). Issuer only.
    pub fn set_state(ctx: Context<SetState>, state: ClaimState) -> Result<()> {
        let r = &mut ctx.accounts.request;
        let ok = match (r.state, state) {
            (ClaimState::Attested | ClaimState::PartiallyFilled, ClaimState::Frozen) => true,
            (ClaimState::Frozen, ClaimState::Attested) => r.filled_qty == 0,
            (ClaimState::Frozen, ClaimState::PartiallyFilled) => r.filled_qty > 0,
            (ClaimState::Frozen, ClaimState::Restructured) => true,
            _ => false,
        };
        require!(ok, QcError::WrongState);
        r.state = state;
        emit!(StateChanged {
            request: r.key(),
            state,
        });
        Ok(())
    }
}

fn pay_out<'info>(
    queue: &Account<'info, Queue>,
    signer: &UncheckedAccount<'info>,
    from: &InterfaceAccount<'info, TokenAccount>,
    to: &InterfaceAccount<'info, TokenAccount>,
    mint: &InterfaceAccount<'info, Mint>,
    tp: &Interface<'info, TokenInterface>,
    amount: u64,
) -> Result<()> {
    let seeds: &[&[u8]] = &[SIGNER_SEED, &[queue.signer_bump]];
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            tp.key(),
            TransferChecked {
                from: from.to_account_info(),
                mint: mint.to_account_info(),
                to: to.to_account_info(),
                authority: signer.to_account_info(),
            },
            &[seeds],
        ),
        amount,
        mint.decimals,
    )
}

// ── state ───────────────────────────────────────────────────────────────

/// QC-P2 states (doc 06 §5). Transferable / Pledgeable are the
/// independent consent flags on the request.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum ClaimState {
    Requested,
    Attested,
    PartiallyFilled,
    Retired,
    Cancelled,
    Frozen,
    Restructured,
}

/// One attested expected fill: `qty` base units in window `period`
/// (0 = next window).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct ScheduledFill {
    pub period: u32,
    pub qty: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct PledgeRef {
    pub facility_config: Pubkey,
    pub rails_facility: Pubkey,
    pub borrower: Pubkey,
}

#[account]
#[derive(InitSpace)]
pub struct Queue {
    pub authority: Pubkey,
    pub attester: Pubkey,
    pub asset_mint: Pubkey,
    pub usdc_mint: Pubkey,
    pub requests: u64,
    pub bump: u8,
    pub signer_bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Request {
    pub queue: Pubkey,
    pub id: u64,
    pub holder: Pubkey,
    pub qty: u64,
    pub window_date: i64,
    pub request_ts: i64,
    pub rank: u32,
    #[max_len(MAX_SCHEDULE)]
    pub schedule: Vec<ScheduledFill>,
    pub npcs_vault: Pubkey,
    pub cash_vault: Pubkey,
    pub claim_mint: Pubkey,
    /// The token account the claim was minted to.
    pub holder_claim: Pubkey,
    pub transferable: bool,
    pub pledgeable: bool,
    pub state: ClaimState,
    pub filled_qty: u64,
    pub cash_received: u64,
    pub pledge: Option<PledgeRef>,
    pub bump: u8,
}

// ── accounts ────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct InitQueue<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Queue::INIT_SPACE,
        seeds = [QUEUE_SEED, asset_mint.key().as_ref()],
        bump
    )]
    pub queue: Account<'info, Queue>,
    pub asset_mint: InterfaceAccount<'info, Mint>,
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub authority: Signer<'info>,
    /// This program, to locate its ProgramData. The first caller of init_queue would own the queue and name its attester: only the upgrade authority may create one.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, crate::program::QueueClaim>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ QcError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RequestRedemption<'info> {
    #[account(mut, seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(
        init,
        payer = holder,
        space = 8 + Request::INIT_SPACE,
        seeds = [REQUEST_SEED, queue.key().as_ref(), &queue.requests.to_le_bytes()],
        bump
    )]
    pub request: Box<Account<'info, Request>>,
    #[account(
        init,
        payer = holder,
        seeds = [NPCS_VAULT_SEED, request.key().as_ref()],
        bump,
        token::mint = asset_mint,
        token::authority = signer,
        token::token_program = asset_token_program,
    )]
    pub npcs_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the signer PDA (vault authority).
    #[account(seeds = [SIGNER_SEED], bump = queue.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub holder: Signer<'info>,
    #[account(mut, token::mint = asset_mint, token::authority = holder)]
    pub holder_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = queue.asset_mint @ QcError::MintMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Attest<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump, has_one = attester @ QcError::NotAttester)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, has_one = queue @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    #[account(
        init,
        payer = attester,
        seeds = [CLAIM_MINT_SEED, request.key().as_ref()],
        bump,
        mint::decimals = 0,
        mint::authority = signer,
        mint::freeze_authority = signer,
        mint::token_program = claim_token_program,
    )]
    pub claim_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init,
        payer = attester,
        seeds = [CLAIM_ACCOUNT_SEED, request.key().as_ref()],
        bump,
        token::mint = claim_mint,
        token::authority = holder,
        token::token_program = claim_token_program,
    )]
    pub holder_claim: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        init,
        payer = attester,
        seeds = [CASH_VAULT_SEED, request.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = signer,
        token::token_program = token_program,
    )]
    pub cash_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the request's holder (claim token owner).
    #[account(address = request.holder @ QcError::NotHolder)]
    pub holder: UncheckedAccount<'info>,
    /// CHECK: the signer PDA (mint + freeze authority).
    #[account(seeds = [SIGNER_SEED], bump = queue.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub attester: Signer<'info>,
    #[account(address = queue.usdc_mint @ QcError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub claim_token_program: Interface<'info, TokenInterface>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetFlags<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump, has_one = authority @ QcError::NotIssuer)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, has_one = queue @ QcError::MintMismatch, has_one = claim_mint @ QcError::MintMismatch, has_one = holder_claim @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    #[account(mut)]
    pub claim_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub holder_claim: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the signer PDA (freeze authority).
    #[account(seeds = [SIGNER_SEED], bump = queue.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    pub authority: Signer<'info>,
    pub claim_token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Pledge<'info> {
    #[account(mut, has_one = holder @ QcError::NotHolder)]
    pub request: Box<Account<'info, Request>>,
    pub holder: Signer<'info>,
    /// CHECK: liquidity-facility (address-checked in the handler).
    pub facility_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct RecordFill<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump, has_one = authority @ QcError::NotIssuer)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, has_one = queue @ QcError::MintMismatch, has_one = npcs_vault @ QcError::MintMismatch, has_one = cash_vault @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    #[account(mut)]
    pub npcs_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub cash_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the signer PDA.
    #[account(seeds = [SIGNER_SEED], bump = queue.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    pub authority: Signer<'info>,
    #[account(mut)]
    pub issuer_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub issuer_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = queue.asset_mint @ QcError::MintMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = queue.usdc_mint @ QcError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Collect<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(has_one = queue @ QcError::MintMismatch, has_one = cash_vault @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    /// The caller's claim token account: holds the claim.
    #[account(
        constraint = claim.mint == request.claim_mint @ QcError::ClaimNotHeld,
        constraint = claim.owner == holder.key() @ QcError::ClaimNotHeld,
        constraint = claim.amount == 1 @ QcError::ClaimNotHeld
    )]
    pub claim: Box<InterfaceAccount<'info, TokenAccount>>,
    pub holder: Signer<'info>,
    #[account(mut)]
    pub cash_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = usdc_mint)]
    pub holder_usdc: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the signer PDA.
    #[account(seeds = [SIGNER_SEED], bump = queue.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(address = queue.usdc_mint @ QcError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct SweepPledged<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, has_one = queue @ QcError::MintMismatch, has_one = cash_vault @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    pub cash_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the signer PDA (proceeds authority).
    #[account(seeds = [SIGNER_SEED], bump = queue.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    /// CHECK: payment-rails (address-checked in the handler).
    pub rails_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Discharge<'info> {
    #[account(mut)]
    pub request: Box<Account<'info, Request>>,
    /// CHECK: the rails advance PDA of the pledge (checked in the handler).
    pub advance: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct QuotePv<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(has_one = queue @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    /// The underlying asset's mark-engine feed.
    #[account(constraint = asset_feed.asset_mint == queue.asset_mint @ QcError::MintMismatch)]
    pub asset_feed: Box<Account<'info, Feed>>,
}

#[derive(Accounts)]
pub struct Cancel<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, has_one = queue @ QcError::MintMismatch, has_one = holder @ QcError::NotHolder, has_one = npcs_vault @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    #[account(mut)]
    pub holder: Signer<'info>,
    #[account(mut)]
    pub npcs_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = asset_mint)]
    pub holder_asset: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, constraint = holder_claim.mint == request.claim_mint @ QcError::ClaimNotHeld)]
    pub holder_claim: Option<Box<InterfaceAccount<'info, TokenAccount>>>,
    #[account(mut, address = request.claim_mint @ QcError::MintMismatch)]
    pub claim_mint: Option<Box<InterfaceAccount<'info, Mint>>>,
    /// CHECK: the signer PDA.
    #[account(seeds = [SIGNER_SEED], bump = queue.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    #[account(address = queue.asset_mint @ QcError::MintMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    pub claim_token_program: Option<Interface<'info, TokenInterface>>,
}

#[derive(Accounts)]
pub struct SetState<'info> {
    #[account(has_one = authority @ QcError::NotIssuer)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, has_one = queue @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    pub authority: Signer<'info>,
}

// ── events ──────────────────────────────────────────────────────────────

#[event]
pub struct QueueConfigured {
    pub queue: Pubkey,
    pub authority: Pubkey,
    pub attester: Pubkey,
}

#[event]
pub struct RequestSubmitted {
    pub request: Pubkey,
    pub holder: Pubkey,
    pub qty: u64,
    pub window_date: i64,
}

#[event]
pub struct RequestAttested {
    pub request: Pubkey,
    pub rank: u32,
    pub claim_mint: Pubkey,
}

#[event]
pub struct FlagsSet {
    pub request: Pubkey,
    pub transferable: bool,
    pub pledgeable: bool,
}

#[event]
pub struct ClaimPledged {
    pub request: Pubkey,
    pub facility: Pubkey,
    pub borrower: Pubkey,
    pub amount: u64,
}

#[event]
pub struct FillRecorded {
    pub request: Pubkey,
    pub fill_qty: u64,
    pub price: u64,
    pub cash: u64,
    pub filled_qty: u64,
    pub state: ClaimState,
}

#[event]
pub struct ProceedsPaid {
    pub request: Pubkey,
    pub to: Pubkey,
    pub amount: u64,
}

#[event]
pub struct PledgedProceedsSwept {
    pub request: Pubkey,
    pub amount: u64,
    pub discharged: bool,
}

#[event]
pub struct ClaimPv {
    pub request: Pubkey,
    pub pv: u64,
    pub discount_bps_per_period: u64,
    pub mark_price: u64,
    pub at: i64,
}

#[event]
pub struct StateChanged {
    pub request: Pubkey,
    pub state: ClaimState,
}

// ── errors (append-only) ────────────────────────────────────────────────

#[error_code]
pub enum QcError {
    #[msg("amount must be positive")]
    ZeroAmount,
    #[msg("signer is not the queue's attester")]
    NotAttester,
    #[msg("signer is not the issuer authority")]
    NotIssuer,
    #[msg("signer is not the request holder")]
    NotHolder,
    #[msg("mint or vault does not match the queue/request")]
    MintMismatch,
    #[msg("request is not in a state that allows this")]
    WrongState,
    #[msg("schedule empty, too long, or above the request qty")]
    BadSchedule,
    #[msg("issuer has not consented to pledging")]
    NotPledgeable,
    #[msg("claim already pledged")]
    AlreadyPledged,
    #[msg("claim is pledged: proceeds go through sweep_pledged")]
    Pledged,
    #[msg("claim is not pledged")]
    NotPledged,
    #[msg("fill exceeds the unfilled quantity")]
    OverFill,
    #[msg("caller does not hold the claim token")]
    ClaimNotHeld,
    #[msg("facility / rails accounts do not match")]
    FacilityAccounts,
    #[msg("arithmetic overflow")]
    MathOverflow,
    #[msg("Only the program's upgrade authority may do this")]
    NotUpgradeAuthority,
}
