//! `credit-gate` — Gate Program for Token ACL (sRFC37).
//!
//! SRS 08 §4.1 (CG-P1..P7) · doc 09. Audit-scope-critical: this program is
//! the one that enforces law.
//!
//! P0 scope (this skeleton): the CG-P2 config anchor — one governed,
//! evented policy PDA per mint — plus the event families (CG-P7) that every
//! later instruction must emit. CG-P1/P3–P6 (Token ACL validate, SAS
//! attestation read, lot ledger, governor, block-list) land in P1 on this
//! frame. This file carries no `unsafe` and no external program CPIs yet;
//! the sRFC37 gate interface lands in P1 behind a lint-gated boundary.

#![deny(unsafe_code)]
// missing_docs activates in P1 when the hand-written account surface
// dominates; anchor-lang 1.1 macro-generated items trigger it spuriously.
#![allow(missing_docs)]
// anchor-lang's generated code emits cfgs (custom-heap, anchor-debug, …)
// declared in ITS crate, so rustc flags them here as unexpected under
// `-D warnings`. They are expected; P1 adds a build.rs check-cfg allowlist.
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;

declare_id!("7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c");

/// Maximum accepted attester authorities per market (H3: multi-source so a
/// single provider outage cannot halt onboarding).
pub const MAX_ATTESTERS: usize = 4;

/// One immutable-per-market policy configuration (CG-P2).
///
/// Seeded per-mint. Every consumer (auction-engine sessions, facility
/// underwriting) snapshots the fields it depends on — the gate never
/// mutates config mid-session (L5 snapshot discipline). Updates are
/// governed (Squads 2-of-2) and always evented (CG-P7).
#[account]
#[derive(InitSpace)]
pub struct MarketConfig {
    /// The gated mint this config governs.
    pub mint: Pubkey,
    /// Squads 2-of-2: sole authority for this PDA. All updates propose here.
    pub gate_config_authority: Pubkey,
    /// Policy class (pilot: `PILOT_506C_TEST`) — stable u32 for consumers.
    pub policy_class: u32,
    /// Accepted SAS identity schema (CG-P4/P6).
    pub identity_schema: Pubkey,
    /// Accepted SAS accreditation schema (CG-P6).
    pub accreditation_schema: Pubkey,
    /// Attester authorities accepted for these schemas.
    pub attesters: [Pubkey; MAX_ATTESTERS],
    /// Active count into `attesters`.
    pub attester_count: u8,
    /// Distinct investor_id cap (pilot: 8, CG-P4).
    pub holder_cap: u32,
    /// Seasoning seconds (pilot: 7 days — compressed Rule-144 analog, CG-1).
    pub seasoning_seconds: i64,
    /// Attestation must remain valid at least this long after validation (H4).
    pub settlement_horizon_seconds: i64,
    /// Block-list PDA (CG-P5). `None` in the pilot; the check path still exists.
    pub blocklist: Option<Pubkey>,
    /// PDA bump.
    pub bump: u8,
}

impl MarketConfig {
    /// PDA seed prefix.
    pub const SEED: &'static [u8] = b"config";
}

/// CG-P7: every state change emits exactly one event from these families.
#[event]
#[derive(Debug)]
pub struct ConfigUpdated {
    /// Which field group changed (off-chain schema maps the code).
    pub field_group: u8,
    /// SHA-256 of the post-update config (`MarketConfig::fingerprint`):
    /// every governed field, domain-separated. No PII.
    pub new_config_hash: [u8; 32],
    /// Authority that performed the update.
    pub authority: Pubkey,
}

/// Emitted when a thaw/at-rest validate decision is made (P1: CG-P1).
#[event]
#[derive(Debug)]
pub struct ValidateDecision {
    /// Rejection reason; `None` = approved.
    pub reject: Option<GateReject>,
    /// The gated mint.
    pub mint: Pubkey,
    /// Owner whose account was validated.
    pub owner: Pubkey,
    /// Hash of the attestation inputs used (privacy: no PII on-chain).
    pub attestation_inputs_hash: [u8; 32],
}

/// Emitted when lots are appended/split/merged (P1: CG-P3).
#[event]
#[derive(Debug)]
pub struct LotEvent {
    /// 0 = append, 1 = split, 2 = merge-to-free.
    pub kind: u8,
    /// Gated mint.
    pub mint: Pubkey,
    /// Lot owner.
    pub owner: Pubkey,
    /// Quantity affected (base units).
    pub qty: u64,
    /// New seasoning expiry for appended/split halves.
    pub restriction_expiry: i64,
}

/// Typed rejection reasons (CG-P7). Wire-stable; human-readable mapping
/// lives off-chain (ME-5/UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum GateReject {
    /// Owner wallet is on the enforced block-list.
    Blocklisted,
    /// Missing/expired/mismatched-attester attestation for the owner.
    AttestationInvalid,
    /// Attestation expires before the configured settlement horizon (H4).
    AttestationExpiry,
    /// Jurisdiction flag rejected by the market policy class.
    JurisdictionRejected,
    /// Allocating a new investor_id would breach the governor cap (CG-P4).
    GovernorCap,
    /// A required market config field is unset/misconfigured.
    ConfigInvalid,
    /// Lot ledger full and uncompacted (D15 boundary).
    LotLedgerFull,
}

/// Error set for the gate program.
#[error_code]
pub enum GateError {
    /// Signer is not the config's governing authority.
    #[msg("Signer is not the gate config authority")]
    Unauthorized,
    /// More attesters supplied than the fixed slot count.
    #[msg("Too many attesters (max 4)")]
    TooManyAttesters,
    /// Config blocklist was expected to be set but is not.
    #[msg("Config field required by this operation is not set")]
    ConfigFieldMissing,
    /// `initialize_config` signer is not this program's upgrade authority.
    /// Appended (never renumber): codes above are wire-stable.
    #[msg("Signer is not the program upgrade authority")]
    NotUpgradeAuthority,
}

#[program]
pub mod credit_gate {
    use super::*;

    /// CG-P2: initialize the per-mint market config. Called once per gated
    /// mint by the gate-config authority (Squads 2-of-2). The signer must be
    /// this program's upgrade authority, so the first-caller-wins `init`
    /// cannot be front-run by anyone watching for a new mint (#26).
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_config(
        ctx: Context<InitializeConfig>,
        policy_class: u32,
        holder_cap: u32,
        seasoning_seconds: i64,
        settlement_horizon_seconds: i64,
        attesters: Vec<Pubkey>,
        identity_schema: Pubkey,
        accreditation_schema: Pubkey,
    ) -> Result<()> {
        require!(
            attesters.len() <= MAX_ATTESTERS,
            GateError::TooManyAttesters
        );
        // Extract keys before the mutable account borrow (borrow discipline).
        let mint_key = ctx.accounts.mint.key();
        let authority_key = ctx.accounts.authority.key();
        let bump = ctx.bumps.config;
        let cfg = &mut ctx.accounts.config;
        let mut atts = [Pubkey::default(); MAX_ATTESTERS];
        atts[..attesters.len()].copy_from_slice(&attesters);
        cfg.set_inner(MarketConfig {
            mint: mint_key,
            gate_config_authority: authority_key,
            policy_class,
            identity_schema,
            accreditation_schema,
            attesters: atts,
            attester_count: attesters.len() as u8,
            holder_cap,
            seasoning_seconds,
            settlement_horizon_seconds,
            blocklist: None,
            bump,
        });
        emit!(ConfigUpdated {
            field_group: 0,
            new_config_hash: cfg.fingerprint(),
            authority: authority_key,
        });
        Ok(())
    }

    /// CG-P2: governed field updates. Every call emits `ConfigUpdated`.
    /// Snapshot discipline: auction-engine snapshots config at session open;
    /// the gate itself never mutates config mid-session (L5).
    #[allow(clippy::too_many_arguments)]
    pub fn update_config(
        ctx: Context<UpdateConfig>,
        policy_class: Option<u32>,
        holder_cap: Option<u32>,
        seasoning_seconds: Option<i64>,
        settlement_horizon_seconds: Option<i64>,
        attester_add: Option<Pubkey>,
        attester_remove: Option<Pubkey>,
        blocklist_set: Option<Pubkey>,
    ) -> Result<()> {
        let authority_key = ctx.accounts.gate_config_authority.key();
        let cfg = &mut ctx.accounts.config;
        if let Some(v) = policy_class {
            cfg.policy_class = v;
        }
        if let Some(v) = holder_cap {
            cfg.holder_cap = v;
        }
        if let Some(v) = seasoning_seconds {
            cfg.seasoning_seconds = v;
        }
        if let Some(v) = settlement_horizon_seconds {
            cfg.settlement_horizon_seconds = v;
        }
        if let Some(add) = attester_add {
            let n = cfg.attester_count as usize;
            if n < MAX_ATTESTERS && !cfg.attesters[..n].contains(&add) {
                cfg.attesters[n] = add;
                cfg.attester_count += 1;
            }
        }
        if let Some(remove) = attester_remove {
            let n = cfg.attester_count as usize;
            if let Some(idx) = cfg.attesters[..n].iter().position(|a| *a == remove) {
                cfg.attesters[idx] = cfg.attesters[n - 1];
                cfg.attesters[n - 1] = Pubkey::default();
                cfg.attester_count -= 1;
            }
        }
        if let Some(bl) = blocklist_set {
            cfg.blocklist = Some(bl);
        }
        emit!(ConfigUpdated {
            field_group: 1,
            new_config_hash: cfg.fingerprint(),
            authority: authority_key,
        });
        Ok(())
    }
}

/// Domain separator for [`MarketConfig::fingerprint`]. Bump the version if
/// the field encoding below ever changes.
pub const CONFIG_HASH_DOMAIN: &[u8] = b"nundina.credit-gate.MarketConfig.v1";

impl MarketConfig {
    /// SHA-256 over every governed field, for event provenance (CG-P7).
    ///
    /// Covered: mint, gate_config_authority, policy_class, identity_schema,
    /// accreditation_schema, all `MAX_ATTESTERS` attester slots,
    /// attester_count, holder_cap, seasoning_seconds,
    /// settlement_horizon_seconds and blocklist (a 0/1 tag, then the key).
    /// Not covered: `bump`. It is PDA derivation metadata, not governed
    /// state, which matches `payment-rails`' `RailsConfig::fingerprint`.
    /// Any change to a governed field changes the hash (#49).
    pub fn fingerprint(&self) -> [u8; 32] {
        let blocklist_tag = [u8::from(self.blocklist.is_some())];
        let blocklist_key = self.blocklist.unwrap_or_default();
        let (policy, cap) = (
            self.policy_class.to_le_bytes(),
            self.holder_cap.to_le_bytes(),
        );
        let (season, horizon) = (
            self.seasoning_seconds.to_le_bytes(),
            self.settlement_horizon_seconds.to_le_bytes(),
        );
        let mut parts: Vec<&[u8]> = vec![
            CONFIG_HASH_DOMAIN,
            self.mint.as_ref(),
            self.gate_config_authority.as_ref(),
            &policy,
            self.identity_schema.as_ref(),
            self.accreditation_schema.as_ref(),
            core::slice::from_ref(&self.attester_count),
            &cap,
            &season,
            &horizon,
            &blocklist_tag,
            blocklist_key.as_ref(),
        ];
        parts.extend(self.attesters.iter().map(|a| a.as_ref()));
        solana_sha256_hasher::hashv(&parts).to_bytes()
    }
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    /// Per-mint policy PDA (CG-P2).
    #[account(
        init,
        payer = authority,
        space = 8 + MarketConfig::INIT_SPACE,
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump
    )]
    pub config: Account<'info, MarketConfig>,
    /// CHECK: mint key is used for PDA seeding; token-type validation of
    /// this mint happens at ACL wiring in P1 (CG-P1).
    pub mint: UncheckedAccount<'info>,
    /// Upgrade authority (Squads 2-of-2); becomes the gate-config authority
    /// and pays rent.
    #[account(mut)]
    pub authority: Signer<'info>,
    /// This program — used only to locate its ProgramData account.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, program::CreditGate>,
    /// Holds the upgrade authority the signer must match.
    #[account(
        constraint = program_data.upgrade_authority_address == Some(authority.key())
            @ GateError::NotUpgradeAuthority
    )]
    pub program_data: Account<'info, ProgramData>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    /// Per-mint policy PDA; only its governing authority may update.
    #[account(
        mut,
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    /// CHECK: mint key must derive the config PDA above.
    pub mint: UncheckedAccount<'info>,
    /// Squads 2-of-2 authority (checked by `has_one`).
    pub gate_config_authority: Signer<'info>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MarketConfig {
        MarketConfig {
            mint: Pubkey::new_from_array([1; 32]),
            gate_config_authority: Pubkey::new_from_array([2; 32]),
            policy_class: 1,
            identity_schema: Pubkey::new_from_array([3; 32]),
            accreditation_schema: Pubkey::new_from_array([4; 32]),
            attesters: [
                Pubkey::new_from_array([5; 32]),
                Pubkey::default(),
                Pubkey::default(),
                Pubkey::default(),
            ],
            attester_count: 1,
            holder_cap: 8,
            seasoning_seconds: 604_800,
            settlement_horizon_seconds: 86_400,
            blocklist: None,
            bump: 255,
        }
    }

    #[test]
    fn fingerprint_tracks_every_governed_field() {
        let base = cfg().fingerprint();
        assert_eq!(base, cfg().fingerprint(), "deterministic");
        let k = Pubkey::new_from_array([9; 32]);
        let mut last_slot = cfg();
        last_slot.attesters[3] = k;
        let variants = [
            MarketConfig { mint: k, ..cfg() },
            MarketConfig {
                gate_config_authority: k,
                ..cfg()
            },
            MarketConfig {
                policy_class: 2,
                ..cfg()
            },
            MarketConfig {
                identity_schema: k,
                ..cfg()
            },
            MarketConfig {
                accreditation_schema: k,
                ..cfg()
            },
            last_slot,
            MarketConfig {
                attester_count: 2,
                ..cfg()
            },
            MarketConfig {
                holder_cap: 9,
                ..cfg()
            },
            MarketConfig {
                seasoning_seconds: 604_801,
                ..cfg()
            },
            MarketConfig {
                settlement_horizon_seconds: 86_401,
                ..cfg()
            },
            MarketConfig {
                blocklist: Some(k),
                ..cfg()
            },
            // `Some(default)` must not collide with `None`.
            MarketConfig {
                blocklist: Some(Pubkey::default()),
                ..cfg()
            },
        ];
        for (i, v) in variants.iter().enumerate() {
            assert_ne!(v.fingerprint(), base, "variant {i} did not change the hash");
        }
        // The bump is derivation metadata, not governed state.
        assert_eq!(MarketConfig { bump: 1, ..cfg() }.fingerprint(), base);
    }

    #[test]
    fn fingerprint_uses_the_full_32_bytes() {
        let h = cfg().fingerprint();
        assert!(h[8..].iter().any(|b| *b != 0), "not a padded 64-bit hash");
    }
}
