//! #202 — the market compliance surface.
//!
//! - **The tail.** The new per-market fields (issuer flags, ticket floor,
//!   exemption class, affiliate limit) live in a [`Compliance`] record
//!   written *behind* the `MarketConfig` struct in the same account, at
//!   [`Compliance::OFFSET`]. `MarketConfig`'s own layout never changes, so
//!   every program that decodes it (auction-engine, liquidity-facility,
//!   the app and services) keeps working against a market created before
//!   or after #202: `Account<MarketConfig>` reads the struct prefix and
//!   writes back only the prefix. A config without a tail reads
//!   [`Compliance::default`], which is exactly the pre-#202 behaviour;
//!   `migrate_config` (permissionless) reallocs it to carry one.
//! - **`update_compliance`** (gate-config authority): the governed setter
//!   of the tail, evented as `ConfigUpdated` with `field_group_ext`.
//! - **Affiliates (doc 07 L6).** `register_affiliate` marks a wallet an
//!   issuer affiliate on one market ([`AffiliateRecord`]); `settle_check`
//!   then counts that seller's fills against `affiliate_volume_limit` per
//!   `affiliate_window_seconds`. Windows are tumbling (a window opens at the
//!   first sale after the previous one ended), which is the "simple"
//!   limit: across a boundary up to twice the limit can sell inside one
//!   window's length. Rule 144(e)'s own measure (greater of 1 % of the
//!   class or four weeks' average volume, per three months) is counsel's
//!   to translate into the two parameters (Q4).
//! - **Seizure trail (doc 09 Path 7, D14).** `record_seizure` is the
//!   evented hook for a permanent-delegate transfer: Token-2022 moves the
//!   units, the gate records who, how much and why (a hash).

use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::eligibility::{Accreditation, Identity};
use crate::{
    field_group_ext, ConfigUpdated, ExemptionClass, GateError, GateReject, MarketConfig,
    UpdateConfig, MAX_DURATION_SECONDS, MIN_AFFILIATE_WINDOW_SECONDS,
    REG_S_MIN_DISTRIBUTION_COMPLIANCE_SECONDS,
};

/// Tail layout version.
pub const COMPLIANCE_VERSION: u8 = 1;

/// Domain separator of [`Compliance::fingerprint`].
pub const COMPLIANCE_HASH_DOMAIN: &[u8] = b"nundina.credit-gate.Compliance.v1";

/// The market's compliance tail (#202), behind `MarketConfig` in the
/// config account. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize, InitSpace)]
pub struct Compliance {
    /// [`COMPLIANCE_VERSION`].
    pub version: u8,
    /// L15 / CG-6: the issuer's reporting (Form D / 4(a)(7) information)
    /// is current. `false` ⇒ the market is disabled for resales:
    /// `settle_check` and investor `custody_sale` refuse every fill
    /// ([`GateError::IssuerReportingStale`]). Governance clears it when
    /// the issuer's attestation lapses. Default `true`, so existing
    /// markets keep trading; set at onboarding.
    pub issuer_reporting_current: bool,
    /// L16 / CG-6: the market's covered persons passed the Rule 506(d)
    /// bad-actor screen. `false` ⇒ disabled like the flag above
    /// ([`GateError::BadActorUnscreened`]). Default `true`, as above.
    pub bad_actor_screened: bool,
    /// CG-4: smallest fill `settle_check` accepts, in base units of the
    /// mint (the gate never sees a price; the 4(a)(7)-style dollar floor is
    /// converted at onboarding). `0` = off, the default.
    pub min_ticket: u64,
    /// CG-3: the market's exemption class. Default [`ExemptionClass::PolicyOnly`].
    pub exemption_class: ExemptionClass,
    /// CG-3 / L27: per-lot distribution-compliance period of a
    /// [`ExemptionClass::RegSNonUs`] market. `0` in every other class.
    pub distribution_compliance_seconds: i64,
    /// L6: most a registered issuer affiliate ([`AffiliateRecord`]) may
    /// sell per `affiliate_window_seconds`, in base units. `0` = off.
    pub affiliate_volume_limit: u64,
    /// L6: the affiliate volume window. `0` while no limit is configured.
    pub affiliate_window_seconds: i64,
    /// Room to append fields without another realloc.
    pub reserved: [u8; 32],
}

impl Default for Compliance {
    fn default() -> Self {
        Compliance {
            version: COMPLIANCE_VERSION,
            issuer_reporting_current: true,
            bad_actor_screened: true,
            min_ticket: 0,
            exemption_class: ExemptionClass::PolicyOnly,
            distribution_compliance_seconds: 0,
            affiliate_volume_limit: 0,
            affiliate_window_seconds: 0,
            reserved: [0; 32],
        }
    }
}

impl Compliance {
    /// Byte offset of the tail: discriminator + `MarketConfig` at its
    /// largest (a `None` block-list serializes shorter, never longer).
    pub const OFFSET: usize = 8 + MarketConfig::INIT_SPACE;
    /// Account size of a config with a tail.
    pub const CONFIG_SPACE: usize = Self::OFFSET + Self::INIT_SPACE;

    /// Does `config` (a `MarketConfig` account) carry a tail?
    pub fn present(config: &AccountInfo) -> bool {
        config.data_len() >= Self::CONFIG_SPACE
    }

    /// The tail of `config`, or the defaults on a config without one.
    pub fn load(config: &AccountInfo) -> Result<Compliance> {
        if !Self::present(config) {
            return Ok(Compliance::default());
        }
        let data = config.try_borrow_data()?;
        let c = Compliance::deserialize(&mut &data[Self::OFFSET..])
            .map_err(|_| error!(GateError::ConfigFieldMissing))?;
        require!(
            c.version == COMPLIANCE_VERSION,
            GateError::ConfigFieldMissing
        );
        Ok(c)
    }

    /// Write the tail into `config` (which must have room for it).
    pub fn store(&self, config: &AccountInfo) -> Result<()> {
        require!(Self::present(config), GateError::ConfigNotMigrated);
        let mut data = config.try_borrow_mut_data()?;
        let mut dst: &mut [u8] = &mut data[Self::OFFSET..];
        self.serialize(&mut dst)
            .map_err(|_| error!(GateError::ConfigFieldMissing))
    }

    /// Bounds every tail satisfies after any write, given its market's
    /// config:
    /// - in a [`ExemptionClass::RegSNonUs`] market,
    ///   `REG_S_MIN_DISTRIBUTION_COMPLIANCE_SECONDS ≤
    ///   distribution_compliance_seconds ≤ seasoning_seconds`: seasoning
    ///   at least as long as the period means quantity merged into FREE
    ///   (which keeps no acquisition date) is past it too. In every other
    ///   class the period is 0.
    /// - with an affiliate limit on, `MIN_AFFILIATE_WINDOW_SECONDS ≤
    ///   affiliate_window_seconds ≤ MAX_DURATION_SECONDS`; with it off, the
    ///   window is anywhere in `0..=MAX_DURATION_SECONDS`.
    pub fn check_bounds(&self, cfg: &MarketConfig) -> Result<()> {
        let dcp_ok = if self.exemption_class == ExemptionClass::RegSNonUs {
            (REG_S_MIN_DISTRIBUTION_COMPLIANCE_SECONDS..=cfg.seasoning_seconds)
                .contains(&self.distribution_compliance_seconds)
        } else {
            self.distribution_compliance_seconds == 0
        };
        let window_floor = if self.affiliate_volume_limit > 0 {
            MIN_AFFILIATE_WINDOW_SECONDS
        } else {
            0
        };
        require!(
            dcp_ok
                && (window_floor..=MAX_DURATION_SECONDS).contains(&self.affiliate_window_seconds),
            GateError::ConfigOutOfBounds
        );
        Ok(())
    }

    /// CG-6 / CG-4: the market-level half of a resale's exemption checklist
    /// — the issuer flags and (when `qty` is `Some`) the ticket floor.
    pub fn check_resale(&self, qty: Option<u64>) -> Result<()> {
        require!(
            self.issuer_reporting_current,
            GateError::IssuerReportingStale
        );
        require!(self.bad_actor_screened, GateError::BadActorUnscreened);
        if let Some(q) = qty {
            require!(q >= self.min_ticket, GateError::BelowMinTicket);
        }
        Ok(())
    }

    /// CG-3 / L27: a lot acquired at `acquired_ts` may be resold at `now`
    /// only after the Reg S distribution-compliance period. Always true
    /// outside [`ExemptionClass::RegSNonUs`].
    pub fn distribution_compliance_met(&self, acquired_ts: i64, now: i64) -> bool {
        self.exemption_class != ExemptionClass::RegSNonUs
            || acquired_ts.saturating_add(self.distribution_compliance_seconds) <= now
    }

    /// CG-3: what the market's [`ExemptionClass`] adds on top of the
    /// policy. It only ever narrows: `PolicyOnly` and `Us506bDataRoom` add
    /// nothing on-chain; `Us506c` requires accreditation whatever the
    /// policy flag says; `RegSNonUs` refuses a `US` identity jurisdiction
    /// (the offshore book, L27) whatever the policy list says; `EuMtfOnly`
    /// admits nobody (routed out in v1–v2, L26).
    pub fn class_reject(
        &self,
        identity: &Identity,
        accreditation: &Accreditation,
    ) -> Option<GateReject> {
        match self.exemption_class {
            ExemptionClass::PolicyOnly | ExemptionClass::Us506bDataRoom => None,
            ExemptionClass::Us506c => {
                (!accreditation.accredited).then_some(GateReject::NotAccredited)
            }
            ExemptionClass::RegSNonUs => {
                (identity.jurisdiction == *b"US").then_some(GateReject::JurisdictionRejected)
            }
            ExemptionClass::EuMtfOnly => Some(GateReject::ExemptionClassClosed),
        }
    }

    /// SHA-256 of the market's governed state, tail included: the config's
    /// own [`MarketConfig::fingerprint`], then every tail field.
    pub fn fingerprint(&self, cfg: &MarketConfig) -> [u8; 32] {
        let flags = [
            u8::from(self.issuer_reporting_current),
            u8::from(self.bad_actor_screened),
            self.exemption_class as u8,
        ];
        solana_sha256_hasher::hashv(&[
            COMPLIANCE_HASH_DOMAIN,
            &cfg.fingerprint(),
            &flags,
            &self.min_ticket.to_le_bytes(),
            &self.distribution_compliance_seconds.to_le_bytes(),
            &self.affiliate_volume_limit.to_le_bytes(),
            &self.affiliate_window_seconds.to_le_bytes(),
        ])
        .to_bytes()
    }
}

/// One registered issuer affiliate on one market, and its sale volume in
/// the current window (L6).
#[account]
#[derive(InitSpace)]
pub struct AffiliateRecord {
    /// The gated mint.
    pub mint: Pubkey,
    /// The affiliate's wallet (a seller).
    pub wallet: Pubkey,
    /// When the current window opened (0 = no sale yet).
    pub window_start: i64,
    /// Base units sold in the current window.
    pub window_qty: u64,
    /// PDA bump.
    pub bump: u8,
}

impl AffiliateRecord {
    /// PDA seed prefix: `["affiliate", mint, wallet]`.
    pub const SEED: &'static [u8] = b"affiliate";

    /// The record address for `(mint, wallet)`.
    pub fn address(mint: &Pubkey, wallet: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[Self::SEED, mint.as_ref(), wallet.as_ref()], &crate::ID).0
    }

    /// Count a sale of `qty` at `now` against `limit` per `window`
    /// seconds. A window that has run out restarts at `now`. Fails
    /// [`GateError::AffiliateVolumeExceeded`] (changing nothing) if the
    /// window's total would pass `limit`.
    pub fn charge(&mut self, qty: u64, now: i64, limit: u64, window: i64) -> Result<()> {
        // No sale yet (`window_start == 0`), or the window ran out: a new
        // window opens at this sale.
        let expired = self.window_start == 0 || now >= self.window_start.saturating_add(window);
        let (start, sold) = if expired {
            (now, 0)
        } else {
            (self.window_start, self.window_qty)
        };
        let total = sold.checked_add(qty).ok_or(GateError::MathOverflow)?;
        require!(total <= limit, GateError::AffiliateVolumeExceeded);
        self.window_start = start;
        self.window_qty = total;
        Ok(())
    }
}

/// `update_compliance` arguments: one `Option` per layout-v2 field, `None`
/// = leave as is (like `update_config`).
#[derive(Debug, Clone, Default, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub struct ComplianceUpdate {
    pub issuer_reporting_current: Option<bool>,
    pub bad_actor_screened: Option<bool>,
    pub min_ticket: Option<u64>,
    pub exemption_class: Option<ExemptionClass>,
    pub distribution_compliance_seconds: Option<i64>,
    pub affiliate_volume_limit: Option<u64>,
    pub affiliate_window_seconds: Option<i64>,
}

impl ComplianceUpdate {
    /// [`field_group_ext`] bits this update writes.
    pub fn mask(&self) -> u8 {
        [
            (
                self.issuer_reporting_current.is_some() || self.bad_actor_screened.is_some(),
                field_group_ext::ISSUER_FLAGS,
            ),
            (self.min_ticket.is_some(), field_group_ext::MIN_TICKET),
            (
                self.exemption_class.is_some() || self.distribution_compliance_seconds.is_some(),
                field_group_ext::EXEMPTION_CLASS,
            ),
            (
                self.affiliate_volume_limit.is_some() || self.affiliate_window_seconds.is_some(),
                field_group_ext::AFFILIATE_LIMIT,
            ),
        ]
        .iter()
        .filter(|(set, _)| *set)
        .fold(0, |m, (_, bit)| m | bit)
    }

    /// Write every supplied field into `cfg` (bounds are the caller's).
    pub fn apply(&self, cfg: &mut Compliance) {
        if let Some(v) = self.issuer_reporting_current {
            cfg.issuer_reporting_current = v;
        }
        if let Some(v) = self.bad_actor_screened {
            cfg.bad_actor_screened = v;
        }
        if let Some(v) = self.min_ticket {
            cfg.min_ticket = v;
        }
        if let Some(v) = self.exemption_class {
            cfg.exemption_class = v;
        }
        if let Some(v) = self.distribution_compliance_seconds {
            cfg.distribution_compliance_seconds = v;
        }
        if let Some(v) = self.affiliate_volume_limit {
            cfg.affiliate_volume_limit = v;
        }
        if let Some(v) = self.affiliate_window_seconds {
            cfg.affiliate_window_seconds = v;
        }
    }
}

/// Emitted by `register_affiliate` / `deregister_affiliate` (CG-P7).
#[event]
#[derive(Debug)]
pub struct AffiliateSet {
    pub mint: Pubkey,
    pub wallet: Pubkey,
    pub registered: bool,
    pub authority: Pubkey,
}

/// Emitted by `record_seizure` (D14, doc 09 Path 7): one per
/// permanent-delegate transfer, so delegate use is indexed.
#[event]
#[derive(Debug)]
pub struct SeizureRecorded {
    /// The gated mint whose permanent delegate moved the units.
    pub mint: Pubkey,
    /// The wallet the units were taken from.
    pub from: Pubkey,
    /// The wallet (or account owner) they went to.
    pub to: Pubkey,
    /// Base units moved.
    pub amount: u64,
    /// Hash of the off-chain basis (court order, sanctions action, …).
    /// Never the reason itself: no PII on-chain.
    pub reason_hash: [u8; 32],
    /// The gate-config authority that recorded it.
    pub authority: Pubkey,
    /// When it was recorded.
    pub recorded_at: i64,
}

/// `migrate_config` body (see the instruction doc in `lib.rs`): realloc
/// the config to carry its tail and write the defaults.
pub fn process_migrate_config(ctx: Context<MigrateConfig>) -> Result<()> {
    let ai = ctx.accounts.config.to_account_info();
    require!(!Compliance::present(&ai), GateError::ConfigAlreadyMigrated);
    let need = Rent::get()?.minimum_balance(Compliance::CONFIG_SPACE);
    let have = ai.lamports();
    if have < need {
        system_program::transfer(
            CpiContext::new(
                system_program::ID,
                system_program::Transfer {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ai.clone(),
                },
            ),
            need - have,
        )?;
    }
    ai.resize(Compliance::CONFIG_SPACE)?;
    let c = Compliance::default();
    c.store(&ai)?;
    let cfg = &ctx.accounts.config;
    emit!(ConfigUpdated {
        mint: cfg.mint,
        field_group: 0,
        new_config_hash: c.fingerprint(cfg),
        authority: ctx.accounts.payer.key(),
        field_group_ext: field_group_ext::MIGRATED,
    });
    Ok(())
}

/// `update_compliance` body.
pub fn process_update_compliance(
    ctx: Context<UpdateConfig>,
    update: ComplianceUpdate,
) -> Result<()> {
    let mask = update.mask();
    require!(mask != 0, GateError::EmptyComplianceUpdate);
    let authority = ctx.accounts.gate_config_authority.key();
    let cfg = &ctx.accounts.config;
    let ai = cfg.to_account_info();
    require!(Compliance::present(&ai), GateError::ConfigNotMigrated);
    let mut c = Compliance::load(&ai)?;
    update.apply(&mut c);
    c.check_bounds(cfg)?;
    c.store(&ai)?;
    emit!(ConfigUpdated {
        mint: cfg.mint,
        field_group: 0,
        new_config_hash: c.fingerprint(cfg),
        authority,
        field_group_ext: mask,
    });
    Ok(())
}

/// `register_affiliate` body.
pub fn process_register_affiliate(ctx: Context<RegisterAffiliate>, wallet: Pubkey) -> Result<()> {
    let mint = ctx.accounts.config.mint;
    ctx.accounts.record.set_inner(AffiliateRecord {
        mint,
        wallet,
        window_start: 0,
        window_qty: 0,
        bump: ctx.bumps.record,
    });
    emit!(AffiliateSet {
        mint,
        wallet,
        registered: true,
        authority: ctx.accounts.gate_config_authority.key(),
    });
    Ok(())
}

/// `deregister_affiliate` body (the record closes by constraint).
pub fn process_deregister_affiliate(ctx: Context<DeregisterAffiliate>) -> Result<()> {
    emit!(AffiliateSet {
        mint: ctx.accounts.record.mint,
        wallet: ctx.accounts.record.wallet,
        registered: false,
        authority: ctx.accounts.gate_config_authority.key(),
    });
    Ok(())
}

/// `record_seizure` body.
pub fn process_record_seizure(
    ctx: Context<RecordSeizure>,
    from: Pubkey,
    to: Pubkey,
    amount: u64,
    reason_hash: [u8; 32],
) -> Result<()> {
    require!(amount > 0, GateError::EmptySeizure);
    emit!(SeizureRecorded {
        mint: ctx.accounts.config.mint,
        from,
        to,
        amount,
        reason_hash,
        authority: ctx.accounts.gate_config_authority.key(),
        recorded_at: Clock::get()?.unix_timestamp,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct MigrateConfig<'info> {
    #[account(mut, seeds = [MarketConfig::SEED, mint.key().as_ref()], bump = config.bump)]
    pub config: Box<Account<'info, MarketConfig>>,
    /// CHECK: mint key must derive the config PDA.
    pub mint: UncheckedAccount<'info>,
    /// Anyone; funds the extra rent.
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(wallet: Pubkey)]
pub struct RegisterAffiliate<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Box<Account<'info, MarketConfig>>,
    #[account(
        init,
        payer = gate_config_authority,
        space = 8 + AffiliateRecord::INIT_SPACE,
        seeds = [AffiliateRecord::SEED, mint.key().as_ref(), wallet.as_ref()],
        bump
    )]
    pub record: Account<'info, AffiliateRecord>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DeregisterAffiliate<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Box<Account<'info, MarketConfig>>,
    #[account(
        mut,
        close = gate_config_authority,
        seeds = [AffiliateRecord::SEED, mint.key().as_ref(), record.wallet.as_ref()],
        bump = record.bump
    )]
    pub record: Account<'info, AffiliateRecord>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct RecordSeizure<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Box<Account<'info, MarketConfig>>,
    /// CHECK: mint key must derive the config PDA.
    pub mint: UncheckedAccount<'info>,
    pub gate_config_authority: Signer<'info>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> AffiliateRecord {
        AffiliateRecord {
            mint: Pubkey::new_from_array([1; 32]),
            wallet: Pubkey::new_from_array([2; 32]),
            window_start: 0,
            window_qty: 0,
            bump: 255,
        }
    }

    const DAY: i64 = 86_400;

    #[test]
    fn affiliate_window_caps_volume_and_restarts() {
        let mut r = rec();
        r.charge(60, 10 * DAY, 100, 90 * DAY).unwrap();
        assert_eq!((r.window_start, r.window_qty), (10 * DAY, 60));
        r.charge(40, 20 * DAY, 100, 90 * DAY).unwrap();
        assert_eq!(r.window_qty, 100, "exactly the limit is allowed");
        let before = (r.window_start, r.window_qty);
        assert!(r.charge(1, 99 * DAY, 100, 90 * DAY).is_err());
        assert_eq!(
            (r.window_start, r.window_qty),
            before,
            "a refusal changes nothing"
        );
        // The window that opened at day 10 ends at day 100.
        r.charge(70, 100 * DAY, 100, 90 * DAY).unwrap();
        assert_eq!((r.window_start, r.window_qty), (100 * DAY, 70));
        assert!(r.charge(31, 101 * DAY, 100, 90 * DAY).is_err());
        assert!(r.charge(u64::MAX, 400 * DAY, u64::MAX, DAY).is_ok());
        assert!(r.charge(1, 400 * DAY, u64::MAX, DAY).is_err(), "overflow");
    }

    #[test]
    fn reg_s_lots_wait_out_their_distribution_period() {
        let mut c = Compliance::default();
        assert!(
            c.distribution_compliance_met(10 * DAY, 10 * DAY),
            "PolicyOnly: no clock"
        );
        c.exemption_class = ExemptionClass::RegSNonUs;
        c.distribution_compliance_seconds = 40 * DAY;
        assert!(!c.distribution_compliance_met(10 * DAY, 49 * DAY));
        assert!(c.distribution_compliance_met(10 * DAY, 50 * DAY));
    }

    #[test]
    fn the_defaults_are_the_pre_202_behaviour() {
        let c = Compliance::default();
        assert!(c.check_resale(Some(1)).is_ok());
        assert_eq!(c.exemption_class, ExemptionClass::PolicyOnly);
        assert_eq!((c.min_ticket, c.affiliate_volume_limit), (0, 0));
    }

    #[test]
    fn compliance_mask_has_one_bit_per_group() {
        use field_group_ext::*;
        assert_eq!(ComplianceUpdate::default().mask(), 0);
        let u = ComplianceUpdate {
            bad_actor_screened: Some(false),
            distribution_compliance_seconds: Some(1),
            ..Default::default()
        };
        assert_eq!(u.mask(), ISSUER_FLAGS | EXEMPTION_CLASS);
        let all = ComplianceUpdate {
            issuer_reporting_current: Some(true),
            bad_actor_screened: Some(true),
            min_ticket: Some(1),
            exemption_class: Some(ExemptionClass::Us506c),
            distribution_compliance_seconds: Some(0),
            affiliate_volume_limit: Some(1),
            affiliate_window_seconds: Some(1),
        };
        assert_eq!(
            all.mask(),
            ISSUER_FLAGS | MIN_TICKET | EXEMPTION_CLASS | AFFILIATE_LIMIT
        );
        assert_eq!(MIGRATED & all.mask(), 0, "MIGRATED is never an update bit");
    }
}
