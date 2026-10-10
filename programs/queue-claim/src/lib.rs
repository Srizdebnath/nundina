//! `queue-claim` — a tokenized redemption-queue position.
//!
//! SRS 08 §4.4 (QC-P1..P5) · doc 06 §5 · #138. The headline primitive: a
//! pro-rated redemption request becomes a dated, priceable claim that can
//! be **sold in an auction session** or **pledged for a USDC advance**.
//!
//! - QC-P1 `request_redemption` escrows the holder's NPCS in a request
//!   vault; only the configured attester (the issuer / transfer-agent
//!   bridge) can `attest` it, with its queue rank and the expected fill
//!   schedule. A queue whose [`QueueConfig`] sets `require_sas` opens a
//!   request only against a SAS `redemption_request` attestation naming it
//!   ([`sas`]); off by default (no config account).
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
//!   publishes into the claim's feed. `quote_pv_model` derives the
//!   expected fills from the published gate %, queue depth and fill
//!   history instead ([`model`], doc 06 §5); `quote_pv` is the fallback.
//! - QC-P4 pledge path: `pledge` hands the token to `liquidity-facility`
//!   for an advance; `record_fill` books each real window execution (cash
//!   in, NPCS out to the issuer) at a price bound to the asset feed's
//!   published NAV; for a treasury-backed asset (NPCS) `record_fill_redeem`
//!   burns the units through npcs-treasury at NAV instead and the treasury
//!   pays the claim's cash vault (doc 09 Path 5/8); `sweep_pledged` sends a pledged claim's
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

pub mod model;
pub mod sas;

pub use model::FillModel;

declare_id!("2EpAnJkvw6FH8j97k7WRE72E7oyarZpf7t2TYa26red4");

pub const SIGNER_SEED: &[u8] = b"qc_signer";
pub const QUEUE_SEED: &[u8] = b"queue";
pub const REQUEST_SEED: &[u8] = b"request";
pub const NPCS_VAULT_SEED: &[u8] = b"npcs";
pub const CASH_VAULT_SEED: &[u8] = b"cash";
pub const CLAIM_MINT_SEED: &[u8] = b"claim_mint";
pub const CLAIM_ACCOUNT_SEED: &[u8] = b"claim_account";
pub const QUEUE_CONFIG_SEED: &[u8] = b"queue_cfg";
/// Expected-fill schedule entries per request.
pub const MAX_SCHEDULE: usize = 8;
/// QC-P4: a recorded fill price may deviate from the asset feed's
/// redemption NAV (`NAV_effective`, Fresh or Warned mark) by at most this.
pub const FILL_PRICE_TOLERANCE_BPS: u64 = 50;
const BPS: u64 = 10_000;

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
        // QC-P1: a SAS-gated queue opens only an attested request.
        if let Some(cfg) = load_config(&ctx.accounts.queue_config)? {
            if cfg.require_sas {
                let att = ctx
                    .accounts
                    .attestation
                    .as_ref()
                    .ok_or(QcError::BadRedemptionAttestation)?;
                let expected = sas::RedemptionRequestData {
                    request_id: ctx.accounts.queue.requests,
                    qty,
                    window_date,
                };
                let data = att.try_borrow_data()?;
                if let Err(reason) = sas::check_request_attestation(
                    att.owner,
                    att.key,
                    &data,
                    &cfg.sas_credential,
                    &cfg.sas_schema,
                    &cfg.sas_attester,
                    &ctx.accounts.holder.key(),
                    &expected,
                    now,
                ) {
                    msg!("redemption attestation rejected: {:?}", reason);
                    return err!(QcError::BadRedemptionAttestation);
                }
            }
        }
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
    /// and receives the redeemed NPCS from the request vault. `price` must
    /// sit within [`FILL_PRICE_TOLERANCE_BPS`] of the asset feed's
    /// redemption NAV (doc 09: redemption at published NAV). A
    /// treasury-backed asset (its mint authority is npcs-treasury's signer)
    /// redeems through `record_fill_redeem` instead.
    pub fn record_fill(ctx: Context<RecordFill>, fill_qty: u64, price: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let r = &ctx.accounts.request;
        require!(
            matches!(r.state, ClaimState::Attested | ClaimState::PartiallyFilled),
            QcError::WrongState
        );
        require!(fill_qty > 0 && price > 0, QcError::ZeroAmount);
        let left = r.qty - r.filled_qty;
        require!(fill_qty <= left, QcError::OverFill);
        require!(
            !treasury_backed(&ctx.accounts.asset_mint),
            QcError::UseTreasuryRedeem
        );
        let nav_price = redemption_price(&ctx.accounts.asset_feed, now)?;
        require!(within_tolerance(price, nav_price), QcError::FillPriceOffNav);
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
        book_fill(&mut ctx.accounts.request, fill_qty, price, cash);
        Ok(())
    }

    /// QC-P4 / doc 09 Path 5 + 8 for a treasury-backed asset (NPCS): the
    /// window's `fill_qty` burns out of the request vault through
    /// npcs-treasury `redeem_from`, which pays `fill_qty × NAV_effective`
    /// USDC straight into the claim's cash vault. No issuer-supplied price:
    /// the treasury reads the asset feed. The queue authority signs as the
    /// treasury's issuer authority (they must be the same key).
    pub fn record_fill_redeem(ctx: Context<RecordFillRedeem>, fill_qty: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let r = &ctx.accounts.request;
        require!(
            matches!(r.state, ClaimState::Attested | ClaimState::PartiallyFilled),
            QcError::WrongState
        );
        require!(fill_qty > 0, QcError::ZeroAmount);
        require!(fill_qty <= r.qty - r.filled_qty, QcError::OverFill);
        let price = redemption_price(&ctx.accounts.asset_feed, now)?;
        let before = ctx.accounts.cash_vault.amount;
        let bump = ctx.accounts.queue.signer_bump;
        let seeds: &[&[u8]] = &[SIGNER_SEED, &[bump]];
        npcs_treasury::cpi::redeem_from(
            CpiContext::new_with_signer(
                ctx.accounts.treasury_program.key(),
                npcs_treasury::cpi::accounts::RedeemFrom {
                    treasury: ctx.accounts.treasury.to_account_info(),
                    vault: ctx.accounts.treasury_vault.to_account_info(),
                    signer: ctx.accounts.treasury_signer.to_account_info(),
                    asset_mint: ctx.accounts.asset_mint.to_account_info(),
                    usdc_mint: ctx.accounts.usdc_mint.to_account_info(),
                    feed: ctx.accounts.asset_feed.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                    holder: ctx.accounts.signer.to_account_info(),
                    holder_asset: ctx.accounts.npcs_vault.to_account_info(),
                    usdc_destination: ctx.accounts.cash_vault.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info(),
                    asset_token_program: ctx.accounts.asset_token_program.to_account_info(),
                },
                &[seeds],
            ),
            fill_qty,
        )?;
        ctx.accounts.cash_vault.reload()?;
        let cash = ctx.accounts.cash_vault.amount.saturating_sub(before);
        book_fill(&mut ctx.accounts.request, fill_qty, price, cash);
        Ok(())
    }

    /// QC-P1 / QC-P3 per-queue settings, in their own PDA so the live
    /// queue accounts keep their layout. Issuer authority only.
    pub fn init_queue_config(
        ctx: Context<InitQueueConfig>,
        params: QueueConfigParams,
    ) -> Result<()> {
        let c = &mut ctx.accounts.config;
        c.queue = ctx.accounts.queue.key();
        c.bump = ctx.bumps.config;
        apply_config(c, &params)
    }

    /// Change the QC-P1 settings (issuer authority).
    pub fn set_queue_config(ctx: Context<SetQueueConfig>, params: QueueConfigParams) -> Result<()> {
        apply_config(&mut ctx.accounts.config, &params)
    }

    /// QC-P3: publish this window's PV model inputs (the issuer authority
    /// or the queue's attester, the transfer-agent bridge).
    pub fn set_fill_model(ctx: Context<SetFillModel>, model: FillModel) -> Result<()> {
        let q = &ctx.accounts.queue;
        let who = ctx.accounts.publisher.key();
        require!(
            who == q.authority || who == q.attester,
            QcError::NotAttester
        );
        require!(model.is_valid(), QcError::BadFillModel);
        let c = &mut ctx.accounts.config;
        c.fill_model = model;
        c.model_ts = Clock::get()?.unix_timestamp;
        emit!(FillModelSet {
            queue: q.key(),
            model,
            at: c.model_ts,
        });
        Ok(())
    }

    /// QC-P3 (doc 06 §5): the claim's model value from the published
    /// [`FillModel`] — `E[fill_t]` from gate %, queue depth and fill
    /// history over the request's outstanding quantity, each priced at
    /// the asset's `MARK_PROTECTIVE` and discounted per period by
    /// `credit_math::pv_of_claim`. Without a published model it fails
    /// [`QcError::NoFillModel`]: use `quote_pv` (the attested schedule).
    pub fn quote_pv_model(ctx: Context<QuotePvModel>, discount_bps_per_period: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let c = &ctx.accounts.config;
        require!(c.model_ts > 0, QcError::NoFillModel);
        let r = &ctx.accounts.request;
        let feed = &ctx.accounts.asset_feed;
        let price = u128::from(feed.pico_to_price(feed.mark_protective(now)?)?);
        let fills: Vec<ExpectedFill> = c
            .fill_model
            .expected_fills(r.qty - r.filled_qty)
            .into_iter()
            .map(|(period, qty)| ExpectedFill {
                period,
                qty_base: u128::from(qty),
                price_micros: price,
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

/// Book a window fill on the request (QC-P2 state, QC-P4 cash).
fn book_fill(r: &mut Account<Request>, fill_qty: u64, price: u64, cash: u64) {
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
}

/// The window's redemption price (micro-USDC per whole unit): the asset
/// feed's `NAV_effective` on a Fresh or Warned mark — the NAV npcs-treasury
/// redeems at (doc 09 I2). Stale / Suspended: no fills (QC-P2 Frozen).
fn redemption_price(feed: &Feed, now: i64) -> Result<u64> {
    require!(feed.allows_liquidation(now), QcError::MarkUnavailable);
    let nav = feed.nav_effective(now)?;
    require!(nav > 0, QcError::MarkUnavailable);
    feed.pico_to_price(nav)
}

/// `|price − nav| ≤ nav × FILL_PRICE_TOLERANCE_BPS`.
pub fn within_tolerance(price: u64, nav_price: u64) -> bool {
    let diff = u128::from(price.abs_diff(nav_price));
    diff * u128::from(BPS) <= u128::from(nav_price) * u128::from(FILL_PRICE_TOLERANCE_BPS)
}

/// The asset is treasury-backed NPCS: its mint authority is npcs-treasury's
/// signer PDA for this mint, so redemption must burn through the treasury.
pub fn treasury_backed(mint: &InterfaceAccount<Mint>) -> bool {
    let treasury = npcs_treasury::treasury_pda(&mint.key()).0;
    let signer = npcs_treasury::signer_pda(&treasury).0;
    mint.mint_authority == Some(signer).into()
}

/// The queue's config, if it has one (pinned by seeds in the accounts).
fn load_config(ai: &UncheckedAccount) -> Result<Option<QueueConfig>> {
    if ai.owner != &crate::ID || ai.data_is_empty() {
        return Ok(None);
    }
    Ok(Some(QueueConfig::try_deserialize(
        &mut &ai.try_borrow_data()?[..],
    )?))
}

fn apply_config(c: &mut Account<QueueConfig>, p: &QueueConfigParams) -> Result<()> {
    require!(
        !p.require_sas
            || (p.sas_credential != Pubkey::default()
                && p.sas_schema != Pubkey::default()
                && p.sas_attester != Pubkey::default()),
        QcError::BadQueueConfig
    );
    c.require_sas = p.require_sas;
    c.sas_credential = p.sas_credential;
    c.sas_schema = p.sas_schema;
    c.sas_attester = p.sas_attester;
    emit!(QueueConfigSet {
        queue: c.queue,
        require_sas: p.require_sas,
        sas_credential: p.sas_credential,
        sas_schema: p.sas_schema,
        sas_attester: p.sas_attester,
    });
    Ok(())
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

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueConfigParams {
    /// QC-P1: requests need a SAS `redemption_request` attestation.
    pub require_sas: bool,
    pub sas_credential: Pubkey,
    pub sas_schema: Pubkey,
    /// The SAS signer of request attestations (issuer / TA bridge).
    pub sas_attester: Pubkey,
}

/// Per-queue settings added after the queues went live (#202): its own
/// PDA `["queue_cfg", queue]`, so `Queue` keeps its layout. No account =
/// every default (SAS off, no PV model).
#[account]
#[derive(InitSpace)]
pub struct QueueConfig {
    pub queue: Pubkey,
    pub require_sas: bool,
    pub sas_credential: Pubkey,
    pub sas_schema: Pubkey,
    pub sas_attester: Pubkey,
    /// QC-P3 model inputs (`set_fill_model`).
    pub fill_model: FillModel,
    /// When the model was published (0 = never).
    pub model_ts: i64,
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
    /// CHECK: the queue's [`QueueConfig`] address, pinned by seeds; empty
    /// = no config (SAS off). Always passed, so it cannot be skipped.
    #[account(seeds = [QUEUE_CONFIG_SEED, queue.key().as_ref()], bump)]
    pub queue_config: UncheckedAccount<'info>,
    /// CHECK: QC-P1 SAS `redemption_request` attestation; decoded and
    /// checked in the handler when the config requires it.
    pub attestation: Option<UncheckedAccount<'info>>,
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
    /// The asset's mark-engine feed: the fill price is bound to its NAV.
    #[account(constraint = asset_feed.asset_mint == queue.asset_mint @ QcError::MintMismatch)]
    pub asset_feed: Box<Account<'info, Feed>>,
}

#[derive(Accounts)]
pub struct RecordFillRedeem<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump, has_one = authority @ QcError::NotIssuer)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, has_one = queue @ QcError::MintMismatch, has_one = npcs_vault @ QcError::MintMismatch, has_one = cash_vault @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    #[account(mut)]
    pub npcs_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub cash_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: the signer PDA (request-vault owner: consents to the burn).
    #[account(seeds = [SIGNER_SEED], bump = queue.signer_bump)]
    pub signer: UncheckedAccount<'info>,
    /// Issuer: the queue authority and the treasury authority.
    pub authority: Signer<'info>,
    #[account(mut, address = queue.asset_mint @ QcError::MintMismatch)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = queue.usdc_mint @ QcError::MintMismatch)]
    pub usdc_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(constraint = asset_feed.asset_mint == queue.asset_mint @ QcError::MintMismatch)]
    pub asset_feed: Box<Account<'info, Feed>>,
    /// CHECK: npcs-treasury's treasury for the asset (checked by the treasury).
    #[account(mut)]
    pub treasury: UncheckedAccount<'info>,
    /// CHECK: the treasury's USDC vault (checked by the treasury).
    #[account(mut)]
    pub treasury_vault: UncheckedAccount<'info>,
    /// CHECK: the treasury's signer PDA (checked by the treasury).
    pub treasury_signer: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub asset_token_program: Interface<'info, TokenInterface>,
    pub treasury_program: Program<'info, npcs_treasury::program::NpcsTreasury>,
}

#[derive(Accounts)]
pub struct InitQueueConfig<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump, has_one = authority @ QcError::NotIssuer)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(
        init,
        payer = authority,
        space = 8 + QueueConfig::INIT_SPACE,
        seeds = [QUEUE_CONFIG_SEED, queue.key().as_ref()],
        bump
    )]
    pub config: Box<Account<'info, QueueConfig>>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetQueueConfig<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump, has_one = authority @ QcError::NotIssuer)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, seeds = [QUEUE_CONFIG_SEED, queue.key().as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, QueueConfig>>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct SetFillModel<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(mut, seeds = [QUEUE_CONFIG_SEED, queue.key().as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, QueueConfig>>,
    /// The issuer authority or the queue's attester.
    pub publisher: Signer<'info>,
}

#[derive(Accounts)]
pub struct QuotePvModel<'info> {
    #[account(seeds = [QUEUE_SEED, queue.asset_mint.as_ref()], bump = queue.bump)]
    pub queue: Box<Account<'info, Queue>>,
    #[account(seeds = [QUEUE_CONFIG_SEED, queue.key().as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, QueueConfig>>,
    #[account(has_one = queue @ QcError::MintMismatch)]
    pub request: Box<Account<'info, Request>>,
    /// The underlying asset's mark-engine feed.
    #[account(constraint = asset_feed.asset_mint == queue.asset_mint @ QcError::MintMismatch)]
    pub asset_feed: Box<Account<'info, Feed>>,
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

#[event]
pub struct QueueConfigSet {
    pub queue: Pubkey,
    pub require_sas: bool,
    pub sas_credential: Pubkey,
    pub sas_schema: Pubkey,
    pub sas_attester: Pubkey,
}

#[event]
pub struct FillModelSet {
    pub queue: Pubkey,
    pub model: FillModel,
    pub at: i64,
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
    #[msg("fill price deviates from the asset feed's redemption NAV")]
    FillPriceOffNav,
    #[msg("treasury-backed asset: redeem the fill through record_fill_redeem")]
    UseTreasuryRedeem,
    #[msg("missing or invalid SAS redemption_request attestation (QC-P1)")]
    BadRedemptionAttestation,
    #[msg("queue config: SAS required without credential, schema and attester")]
    BadQueueConfig,
    #[msg("fill model out of bounds")]
    BadFillModel,
    #[msg("no fill model published: quote with quote_pv")]
    NoFillModel,
    #[msg("no redemption NAV: the asset mark is Stale, Suspended or unpublished")]
    MarkUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_price_tolerance_is_fifty_bps_of_nav() {
        assert!(within_tolerance(1_000_000, 1_000_000));
        assert!(within_tolerance(1_005_000, 1_000_000));
        assert!(within_tolerance(995_000, 1_000_000));
        assert!(!within_tolerance(1_005_001, 1_000_000));
        assert!(!within_tolerance(994_999, 1_000_000));
        assert!(!within_tolerance(1, 0));
        assert!(within_tolerance(0, 0));
    }

    #[test]
    fn treasury_signer_is_derived_from_the_mint() {
        let mint = Pubkey::new_unique();
        let t = npcs_treasury::treasury_pda(&mint).0;
        assert_eq!(
            t,
            Pubkey::find_program_address(&[b"treasury", mint.as_ref()], &npcs_treasury::ID).0
        );
        assert_ne!(npcs_treasury::signer_pda(&t).0, t);
    }

    #[test]
    fn config_space_fits_the_model() {
        // discriminator + queue + flag + 3 keys + model (4×u64 + u32) + ts + bump
        assert_eq!(QueueConfig::INIT_SPACE, 32 + 1 + 96 + 36 + 8 + 1);
    }
}
