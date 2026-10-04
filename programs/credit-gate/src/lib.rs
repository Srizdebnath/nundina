//! `credit-gate` — Gate Program for Token ACL (sRFC37).
//!
//! SRS 08 §4.1 (CG-P1..P7) · doc 09. Audit-scope-critical: this program is
//! the one that enforces law.
//!
//! CG-P1..P7 (#122): the CG-P2 config anchor (one governed, evented policy
//! PDA per mint), the CG-P5 block-list ([`blocklist`]), the CG-P3 lot ledger
//! ([`lots`]), the CG-P6 SAS attestation read and CG-P4 holder governor
//! behind `validate` ([`eligibility`]), and the CG-P1 Token ACL (sRFC37)
//! thaw/freeze hooks ([`token_acl`]), and the AE-P8 settle-time re-check
//! that payment-rails runs on every fill ([`settle`], #133/#134). The crate
//! carries no `unsafe`; its only CPI is to the system program (account
//! creation).

#![deny(unsafe_code)]
// missing_docs activates in P1 when the hand-written account surface
// dominates; anchor-lang 1.1 macro-generated items trigger it spuriously.
#![allow(missing_docs)]
// anchor-lang's generated code emits cfgs (custom-heap, anchor-debug, …)
// declared in ITS crate, so rustc flags them here as unexpected under
// `-D warnings`. They are expected; P1 adds a build.rs check-cfg allowlist.
#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;

pub mod blocklist;
pub mod eligibility;
pub mod lots;
pub mod settle;
pub mod token_acl;

pub use blocklist::*;
pub use eligibility::*;
pub use lots::*;
pub use settle::*;
pub use token_acl::*;

declare_id!("5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX");

/// Maximum accepted attester authorities per market (H3: multi-source so a
/// single provider outage cannot halt onboarding).
pub const MAX_ATTESTERS: usize = 4;

/// Hard ceiling on `holder_cap` (#53, #95). doc 07 L7 (C4) requires the
/// per-asset cap to sit **below** the statutory numbers: 2 000 holders of
/// record is the Exchange Act §12(g) trigger, so the ceiling is one under
/// it and a market can never be configured *at* the trigger. Governance
/// sets the cap below this (pilot: 8). Any other number is a dated decision
/// against Q5 (doc 07), not a constant edit.
pub const MAX_HOLDER_CAP: u32 = 1_999;

/// Floor on `seasoning_seconds` (#95): one day. Zero seasoning makes
/// `acquired + seasoning ≤ now` true at the instant of acquisition, so
/// CG-1 (the compressed Rule-144 analog, doc 09 §2) would be an absent
/// control rather than a compressed one. Pilot value: 7 days.
pub const MIN_SEASONING_SECONDS: i64 = 86_400;

/// Floor on `settlement_horizon_seconds` (#95): one day. **Placeholder.**
/// H4 (doc 06 §7) requires attestation validity to span the whole
/// commit→settle window, so the true floor is the market session length,
/// which has no plan-of-record value until auction-engine lands. One day
/// is the pilot value; raise this to the session length then.
pub const MIN_SETTLEMENT_HORIZON_SECONDS: i64 = 86_400;

/// Hard ceiling on `seasoning_seconds` and `settlement_horizon_seconds`:
/// 5 years. This is a sanity bound against typos and against
/// `timestamp + duration` overflow in later checks, not a policy number.
pub const MAX_DURATION_SECONDS: i64 = 5 * 365 * 86_400;

/// Bits of [`ConfigUpdated::field_group`] (#96): which governed fields an
/// instruction wrote. Wire-stable; append only.
pub mod field_group {
    /// `policy_class`.
    pub const POLICY_CLASS: u8 = 1 << 0;
    /// `holder_cap`.
    pub const HOLDER_CAP: u8 = 1 << 1;
    /// `seasoning_seconds`.
    pub const SEASONING: u8 = 1 << 2;
    /// `settlement_horizon_seconds`.
    pub const SETTLEMENT_HORIZON: u8 = 1 << 3;
    /// `attesters` / `attester_count`.
    pub const ATTESTERS: u8 = 1 << 4;
    /// `blocklist`.
    pub const BLOCKLIST: u8 = 1 << 5;
    /// `identity_schema` / `accreditation_schema` (set at init only).
    pub const SCHEMAS: u8 = 1 << 6;
    /// The config was created by this instruction (`initialize_config`).
    pub const INIT: u8 = 1 << 7;
}

/// One immutable-per-market policy configuration (CG-P2).
///
/// Seeded per-mint. Every consumer (auction-engine sessions, facility
/// underwriting) snapshots the fields it depends on — the gate never
/// mutates config mid-session (L5 snapshot discipline). Updates are
/// governed by `gate_config_authority` and always evented (CG-P7).
#[account]
#[derive(InitSpace)]
pub struct MarketConfig {
    /// The gated mint this config governs.
    pub mint: Pubkey,
    /// Sole authority for this PDA. Starts as the gate root authority that
    /// initialized it (#93; a single bootstrap key on devnet today, see
    /// DEPLOYMENTS.md). Moves to the Squads 2-of-2 vault (SRS 08 §8) only
    /// through `propose_gate_authority` + `accept_gate_authority`.
    pub gate_config_authority: Pubkey,
    /// Policy class (pilot: `PILOT_506C_TEST`) — stable u32 for consumers.
    pub policy_class: u32,
    /// Accepted SAS identity schema (CG-P4/P6). **Storage only (#52):** no
    /// instruction reads a SAS account yet. There is no SAS program id, no
    /// schema definition and no `validate`, so this field enforces nothing.
    pub identity_schema: Pubkey,
    /// Accepted SAS accreditation schema (CG-P6). Storage only (#52).
    pub accreditation_schema: Pubkey,
    /// Attester authorities accepted for these schemas. **Storage only
    /// (#52):** nothing compares any attestation's signer against this set
    /// until `validate` (CG-P1/P6) lands.
    pub attesters: [Pubkey; MAX_ATTESTERS],
    /// Active count into `attesters`.
    pub attester_count: u8,
    /// Distinct investor_id cap (pilot: 8, CG-P4).
    pub holder_cap: u32,
    /// Seasoning seconds (pilot: 7 days — compressed Rule-144 analog, CG-1).
    pub seasoning_seconds: i64,
    /// Attestation must remain valid at least this long after validation (H4).
    pub settlement_horizon_seconds: i64,
    /// Block-list header (CG-P5, [`BlockList`]). `None` = no list
    /// configured: the screen passes. Set by `initialize_blocklist`;
    /// re-pointable but never clearable. Pointing it anywhere but this
    /// program's header for the mint makes every screen fail closed.
    pub blocklist: Option<Pubkey>,
    /// PDA bump.
    pub bump: u8,
}

impl MarketConfig {
    /// PDA seed prefix.
    pub const SEED: &'static [u8] = b"config";

    /// Bounds every config must satisfy after any write (#53). Checked on
    /// init and on every update, so governance cannot move a config out of
    /// them:
    /// - `1 ≤ holder_cap ≤ MAX_HOLDER_CAP`. A zero cap would reject every
    ///   investor; the ceiling sits below the §12(g) trigger.
    /// - `MIN_SEASONING_SECONDS ≤ seasoning_seconds ≤ MAX_DURATION_SECONDS`.
    ///   Zero or negative seasoning would make `acquired + seasoning ≤ now`
    ///   pass vacuously, at the instant of acquisition (#95).
    /// - `MIN_SETTLEMENT_HORIZON_SECONDS ≤ settlement_horizon_seconds ≤
    ///   MAX_DURATION_SECONDS`. A horizon shorter than the commit→settle
    ///   window would make `expiry ≥ now + horizon` satisfy nothing (H4).
    /// - `blocklist`, when set, is not the default key, the mint or the
    ///   config itself.
    pub fn check_bounds(&self, config_key: &Pubkey) -> Result<()> {
        require!(
            (1..=MAX_HOLDER_CAP).contains(&self.holder_cap)
                && (MIN_SEASONING_SECONDS..=MAX_DURATION_SECONDS).contains(&self.seasoning_seconds)
                && (MIN_SETTLEMENT_HORIZON_SECONDS..=MAX_DURATION_SECONDS)
                    .contains(&self.settlement_horizon_seconds),
            GateError::ConfigOutOfBounds
        );
        if let Some(bl) = self.blocklist {
            require!(
                bl != Pubkey::default() && bl != self.mint && bl != *config_key,
                GateError::InvalidBlocklist
            );
        }
        Ok(())
    }
}

/// Who may create `MarketConfig`s (#93). One per program, seeded once by
/// the upgrade authority while the program is still upgradeable.
///
/// `initialize_config` authorizes against `authority` here, not against
/// `ProgramData`, so market onboarding (CG-P2 per mint, G14) survives
/// `solana program finalize`: doc 06 §11 U1 stages the program to immutable,
/// and an initializer pinned to the upgrade authority would die with it.
/// Seed this **before** finalizing — a finalized program with no root can
/// never onboard a market (fail-closed, nobody else can seed it).
#[account]
#[derive(InitSpace)]
pub struct GateRoot {
    /// Signs `initialize_config`; pays its rent and becomes each new
    /// config's first `gate_config_authority`.
    pub authority: Pubkey,
    /// Successor named by `propose_root_authority`; default = none pending.
    pub pending_authority: Pubkey,
    /// PDA bump.
    pub bump: u8,
}

impl GateRoot {
    /// PDA seed: `["gate_root"]` (singleton).
    pub const SEED: &'static [u8] = b"gate_root";
}

/// A pending hand-over of one market's `gate_config_authority` (#50).
/// Exists only between `propose_gate_authority` and accept/cancel.
#[account]
#[derive(InitSpace)]
pub struct PendingGateAuthority {
    /// The gated mint whose config is being handed over.
    pub mint: Pubkey,
    /// The authority that proposed (and paid rent; refunded on close).
    pub proposed_by: Pubkey,
    /// The key that must sign `accept_gate_authority`.
    pub new_authority: Pubkey,
    /// PDA bump.
    pub bump: u8,
}

impl PendingGateAuthority {
    /// PDA seed prefix: `["pending_gate_authority", mint]`.
    pub const SEED: &'static [u8] = b"pending_gate_authority";
}

/// CG-P7: every state change emits exactly one event from these families.
#[event]
#[derive(Debug)]
pub struct ConfigUpdated {
    /// The gated mint whose config this is (#96): the market key, so an
    /// indexer can file the event per market without the config address.
    pub mint: Pubkey,
    /// Bitmask of the fields this instruction wrote ([`field_group`]).
    /// `INIT` plus every field set on creation; on update, one bit per
    /// supplied argument. Never 0: an update that writes nothing is rejected.
    pub field_group: u8,
    /// SHA-256 of the post-update config (`MarketConfig::fingerprint`):
    /// every governed field, domain-separated. No PII.
    pub new_config_hash: [u8; 32],
    /// Authority that performed the update.
    pub authority: Pubkey,
}

/// Emitted by the gate-root instructions (#93). Exactly one per
/// instruction. `kind`: 0 = seeded, 1 = proposed, 2 = cancelled,
/// 3 = accepted.
#[event]
#[derive(Debug)]
pub struct GateRootAuthority {
    /// 0 = seeded, 1 = proposed, 2 = cancelled, 3 = accepted.
    pub kind: u8,
    /// `GateRoot.authority` before this instruction (default when seeded).
    pub old_authority: Pubkey,
    /// The proposed key (proposed / cancelled) or the authority after this
    /// instruction (seeded / accepted).
    pub new_authority: Pubkey,
}

/// Emitted by the gate-authority hand-over instructions (#50). Exactly one
/// per instruction. `kind`: 0 = proposed, 1 = cancelled, 2 = accepted.
/// On `accepted` the config changed: `new_config_hash` is its fingerprint,
/// otherwise it is the unchanged config's fingerprint.
#[event]
#[derive(Debug)]
pub struct GateAuthorityTransfer {
    /// 0 = proposed, 1 = cancelled, 2 = accepted.
    pub kind: u8,
    /// The gated mint.
    pub mint: Pubkey,
    /// `gate_config_authority` before this instruction.
    pub old_authority: Pubkey,
    /// The proposed (or, on accept, the now-current) authority.
    pub new_authority: Pubkey,
    /// `MarketConfig::fingerprint()` after this instruction.
    pub new_config_hash: [u8; 32],
}

/// Typed rejection reasons (CG-P7). Wire-stable; human-readable mapping
/// lives off-chain (ME-5/UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, AnchorSerialize, AnchorDeserialize)]
pub enum GateReject {
    /// Owner wallet is on the enforced block-list ([`blocklist::screen`]).
    Blocklisted,
    /// Missing, closed, malformed or wrongly-signed attestation, or one not
    /// under this market's credential and schema (CG-P6).
    AttestationInvalid,
    /// Attestation expires before the configured settlement horizon (H4).
    AttestationExpiry,
    /// Jurisdiction flag rejected by the market policy class.
    JurisdictionRejected,
    /// Allocating a new investor_id would breach the governor cap (CG-P4).
    GovernorCap,
    /// A required market config field is unset/misconfigured.
    ConfigInvalid,
    /// Lot ledger full of unexpired lots (D15 boundary). Raised today as
    /// [`GateError::LotLedgerFull`].
    LotLedgerFull,
    /// The accreditation attestation says not accredited, and the market's
    /// policy requires it (506(c)).
    NotAccredited,
    /// Identity `kyc_level` below the market policy's minimum.
    KycLevelTooLow,
    /// The wallet is admitted under one `investor_id` and now presents an
    /// identity attestation with another (CG-P4: no silent re-mapping).
    InvestorIdChanged,
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
    /// `initialize_root` signer is not this program's upgrade authority.
    /// Appended (never renumber): codes above are wire-stable.
    #[msg("Signer is not the program upgrade authority")]
    NotUpgradeAuthority,
    /// Proposed gate authority is the default key or the current authority.
    #[msg("Proposed gate authority is invalid")]
    InvalidNewAuthority,
    /// Signer is not the key named by the pending hand-over.
    #[msg("Signer is not the proposed gate authority")]
    NotProposedAuthority,
    /// holder_cap / seasoning / settlement horizon outside the hard bounds.
    #[msg("Config value outside hard bounds")]
    ConfigOutOfBounds,
    /// Block-list target is the default key, the mint or the config itself.
    #[msg("Invalid block-list account")]
    InvalidBlocklist,
    /// Attester already present.
    #[msg("Attester already present")]
    DuplicateAttester,
    /// `attester_remove` names a key that is not an attester.
    #[msg("Attester not found")]
    AttesterNotFound,
    /// `initialize_config` / root rotation signer is not the gate root
    /// authority (#93).
    #[msg("Signer is not the gate root authority")]
    NotRootAuthority,
    /// `update_config` with every argument `None` (#96): nothing to write,
    /// so nothing to event.
    #[msg("Update writes no field")]
    EmptyUpdate,
    /// `accept_root_authority` with no successor proposed.
    #[msg("No gate root hand-over is pending")]
    NoPendingRootAuthority,
    /// The wallet is on the market's block-list (CG-P5).
    #[msg("Wallet is block-listed")]
    Blocklisted,
    /// The config's block-list is not this program's header for the mint,
    /// so the gate cannot read it and fails closed (CG-P5).
    #[msg("Block-list is not readable by the gate")]
    BlocklistUnreadable,
    /// Every lot slot holds an unexpired lot (CG-P3, D15).
    #[msg("Lot ledger full")]
    LotLedgerFull,
    /// No live lot with that id.
    #[msg("Lot not found")]
    LotNotFound,
    /// Zero quantity, or a split that would leave a half empty.
    #[msg("Invalid lot quantity")]
    InvalidLotQty,
    /// `merge_expired` found nothing to merge.
    #[msg("No expired lots")]
    NothingExpired,
    /// Arithmetic overflow.
    #[msg("Math overflow")]
    MathOverflow,
    /// Attestation missing, closed, malformed, mis-signed or off-schema.
    #[msg("Attestation invalid")]
    AttestationInvalid,
    /// Attestation expires inside the settlement horizon (H4).
    #[msg("Attestation expires before the settlement horizon")]
    AttestationExpired,
    /// Jurisdiction not allowed by the market policy.
    #[msg("Jurisdiction rejected")]
    JurisdictionRejected,
    /// A new investor_id would exceed `holder_cap` (CG-P4).
    #[msg("Holder cap reached")]
    GovernorCap,
    /// Not accredited where the policy requires it.
    #[msg("Not accredited")]
    NotAccredited,
    /// KYC level below the policy minimum.
    #[msg("KYC level too low")]
    KycLevelTooLow,
    /// Identity attestation carries a different investor_id.
    #[msg("investor_id changed")]
    InvestorIdChanged,
    /// More jurisdictions than `MAX_JURISDICTIONS`.
    #[msg("Too many jurisdictions")]
    TooManyJurisdictions,
    /// Not an upper-case alpha-2 code, or a duplicate.
    #[msg("Invalid jurisdiction code")]
    InvalidJurisdiction,
    /// Schema account is not the config's SAS schema under the credential.
    #[msg("Invalid SAS schema")]
    InvalidSchema,
    /// Investor record address does not match the attested investor_id.
    #[msg("Investor record mismatch")]
    InvestorRecordMismatch,
    /// A Token ACL hook called outside Token ACL's own thaw/freeze.
    #[msg("Not inside a Token ACL thaw/freeze")]
    NotTokenAclFlow,
    /// Token account is not a Token-2022 account of this mint and owner.
    #[msg("Token account does not match mint/owner")]
    TokenAccountMismatch,
    /// `can_freeze_permissionless` on a holder the gate still admits.
    #[msg("Holder is still eligible")]
    StillEligible,
    /// `update_policy` with nothing to write.
    #[msg("Policy update writes nothing")]
    EmptyPolicyUpdate,
    /// A sale wants more than the seller's seasoned quantity (C2, #133).
    #[msg("Seller's seasoned quantity is short of the sale")]
    SeasoningNotMet,
    /// `settle_check*` not signed by payment-rails' settlement PDA.
    #[msg("Signer is not the payment-rails settlement authority")]
    NotSettlementAuthority,
    /// A fill whose buyer is its seller.
    #[msg("Buyer and seller are the same wallet")]
    SelfTrade,
}

#[program]
pub mod credit_gate {
    use super::*;

    /// Seeds the [`GateRoot`] (#93). One-shot (`init`), and only this
    /// program's upgrade authority may call it, so it cannot be front-run
    /// (#26). It names the root authority directly, so the upgrade key can
    /// seed it straight to a Squads vault it does not control.
    pub fn initialize_root(ctx: Context<InitializeRoot>, authority: Pubkey) -> Result<()> {
        require!(
            authority != Pubkey::default(),
            GateError::InvalidNewAuthority
        );
        ctx.accounts.root.set_inner(GateRoot {
            authority,
            pending_authority: Pubkey::default(),
            bump: ctx.bumps.root,
        });
        emit!(GateRootAuthority {
            kind: 0,
            old_authority: Pubkey::default(),
            new_authority: authority,
        });
        Ok(())
    }

    /// Step 1 of 2 of a root hand-over: the root authority names its
    /// successor. Re-proposing replaces the pending successor.
    pub fn propose_root_authority(ctx: Context<GovernRoot>, new_authority: Pubkey) -> Result<()> {
        let root = &mut ctx.accounts.root;
        require!(
            new_authority != Pubkey::default() && new_authority != root.authority,
            GateError::InvalidNewAuthority
        );
        root.pending_authority = new_authority;
        emit!(GateRootAuthority {
            kind: 1,
            old_authority: root.authority,
            new_authority,
        });
        Ok(())
    }

    /// Withdraws a pending root hand-over. Only the root authority.
    pub fn cancel_root_authority(ctx: Context<GovernRoot>) -> Result<()> {
        let root = &mut ctx.accounts.root;
        let withdrawn = root.pending_authority;
        require!(
            withdrawn != Pubkey::default(),
            GateError::NoPendingRootAuthority
        );
        root.pending_authority = Pubkey::default();
        emit!(GateRootAuthority {
            kind: 2,
            old_authority: root.authority,
            new_authority: withdrawn,
        });
        Ok(())
    }

    /// Step 2 of 2: the proposed key signs to take over the root.
    pub fn accept_root_authority(ctx: Context<AcceptRootAuthority>) -> Result<()> {
        let root = &mut ctx.accounts.root;
        let old_authority = root.authority;
        root.authority = ctx.accounts.new_authority.key();
        root.pending_authority = Pubkey::default();
        emit!(GateRootAuthority {
            kind: 3,
            old_authority,
            new_authority: root.authority,
        });
        Ok(())
    }

    /// CG-P2: initialize the per-mint market config. Called once per gated
    /// mint. The signer must be the [`GateRoot`] authority, so the
    /// first-caller-wins `init` cannot be front-run by anyone watching for a
    /// new mint (#26), and onboarding keeps working after the program is
    /// finalized (#93).
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
        for (i, a) in attesters.iter().enumerate() {
            require!(!attesters[..i].contains(a), GateError::DuplicateAttester);
        }
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
        cfg.check_bounds(&cfg.key())?;
        emit!(ConfigUpdated {
            mint: mint_key,
            field_group: field_group::INIT
                | field_group::POLICY_CLASS
                | field_group::HOLDER_CAP
                | field_group::SEASONING
                | field_group::SETTLEMENT_HORIZON
                | field_group::ATTESTERS
                | field_group::SCHEMAS,
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
        let mask = update_mask(
            policy_class.is_some(),
            holder_cap.is_some(),
            seasoning_seconds.is_some(),
            settlement_horizon_seconds.is_some(),
            attester_add.is_some() || attester_remove.is_some(),
            blocklist_set.is_some(),
        );
        require!(mask != 0, GateError::EmptyUpdate);
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
        // Remove before add, so one call can swap an attester in a full set.
        // Both fail loudly rather than silently emitting an unchanged config.
        if let Some(remove) = attester_remove {
            let n = cfg.attester_count as usize;
            let idx = cfg.attesters[..n]
                .iter()
                .position(|a| *a == remove)
                .ok_or(GateError::AttesterNotFound)?;
            cfg.attesters[idx] = cfg.attesters[n - 1];
            cfg.attesters[n - 1] = Pubkey::default();
            cfg.attester_count -= 1;
        }
        if let Some(add) = attester_add {
            let n = cfg.attester_count as usize;
            require!(
                !cfg.attesters[..n].contains(&add),
                GateError::DuplicateAttester
            );
            require!(n < MAX_ATTESTERS, GateError::TooManyAttesters);
            cfg.attesters[n] = add;
            cfg.attester_count += 1;
        }
        // The block-list can be re-pointed but never cleared: removing the
        // sanctions screen is not a governance action (CG-P5, C11).
        if let Some(bl) = blocklist_set {
            cfg.blocklist = Some(bl);
        }
        cfg.check_bounds(&cfg.key())?;
        emit!(ConfigUpdated {
            mint: cfg.mint,
            field_group: mask,
            new_config_hash: cfg.fingerprint(),
            authority: authority_key,
        });
        Ok(())
    }

    /// Step 1 of 2: the current `gate_config_authority` names its successor
    /// (#50). The upgrade authority has no role here, so a Squads vault can
    /// take over from a key it does not control. One pending proposal per
    /// mint; `cancel_gate_authority` withdraws it.
    pub fn propose_gate_authority(
        ctx: Context<ProposeGateAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        let cfg = &ctx.accounts.config;
        require!(
            new_authority != Pubkey::default() && new_authority != cfg.gate_config_authority,
            GateError::InvalidNewAuthority
        );
        ctx.accounts.pending.set_inner(PendingGateAuthority {
            mint: cfg.mint,
            proposed_by: cfg.gate_config_authority,
            new_authority,
            bump: ctx.bumps.pending,
        });
        emit!(GateAuthorityTransfer {
            kind: 0,
            mint: cfg.mint,
            old_authority: cfg.gate_config_authority,
            new_authority,
            new_config_hash: cfg.fingerprint(),
        });
        Ok(())
    }

    /// Withdraws a pending proposal. Only the current authority may cancel.
    pub fn cancel_gate_authority(ctx: Context<CancelGateAuthority>) -> Result<()> {
        let cfg = &ctx.accounts.config;
        emit!(GateAuthorityTransfer {
            kind: 1,
            mint: cfg.mint,
            old_authority: cfg.gate_config_authority,
            new_authority: ctx.accounts.pending.new_authority,
            new_config_hash: cfg.fingerprint(),
        });
        Ok(())
    }

    /// Step 2 of 2: the proposed key signs to take over. Proving it can sign
    /// before anything moves means a typo'd or unreachable key can never
    /// strand the config.
    pub fn accept_gate_authority(ctx: Context<AcceptGateAuthority>) -> Result<()> {
        let new_authority = ctx.accounts.new_authority.key();
        let cfg = &mut ctx.accounts.config;
        let old_authority = cfg.gate_config_authority;
        cfg.gate_config_authority = new_authority;
        emit!(GateAuthorityTransfer {
            kind: 2,
            mint: cfg.mint,
            old_authority,
            new_authority,
            new_config_hash: cfg.fingerprint(),
        });
        Ok(())
    }

    /// CG-P5: create the market's block-list header and point the config
    /// at it. From then on every screen reads it, and `update_config` can
    /// re-point but never clear it.
    pub fn initialize_blocklist(ctx: Context<InitializeBlocklist>) -> Result<()> {
        let list = ctx.accounts.blocklist.key();
        ctx.accounts.blocklist.set_inner(BlockList {
            mint: ctx.accounts.config.mint,
            entries: 0,
            bump: ctx.bumps.blocklist,
        });
        let cfg = &mut ctx.accounts.config;
        cfg.blocklist = Some(list);
        cfg.check_bounds(&cfg.key())?;
        emit!(ConfigUpdated {
            mint: cfg.mint,
            field_group: field_group::BLOCKLIST,
            new_config_hash: cfg.fingerprint(),
            authority: ctx.accounts.gate_config_authority.key(),
        });
        Ok(())
    }

    /// CG-P5: block `wallet` on this market. Fails if already blocked.
    pub fn block_wallet(
        ctx: Context<BlockWallet>,
        wallet: Pubkey,
        reason_hash: [u8; 32],
    ) -> Result<()> {
        let list = &mut ctx.accounts.blocklist;
        list.entries = list.entries.checked_add(1).ok_or(GateError::MathOverflow)?;
        ctx.accounts.entry.set_inner(BlockEntry {
            mint: list.mint,
            wallet,
            reason_hash,
            blocked_at: Clock::get()?.unix_timestamp,
            bump: ctx.bumps.entry,
        });
        emit!(BlocklistUpdated {
            mint: list.mint,
            wallet,
            blocked: true,
            reason_hash,
            entries: list.entries,
            authority: ctx.accounts.gate_config_authority.key(),
        });
        Ok(())
    }

    /// CG-P5: unblock `wallet` (closes its entry; rent to the authority).
    pub fn unblock_wallet(ctx: Context<UnblockWallet>, wallet: Pubkey) -> Result<()> {
        let list = &mut ctx.accounts.blocklist;
        list.entries = list.entries.saturating_sub(1);
        emit!(BlocklistUpdated {
            mint: list.mint,
            wallet,
            blocked: false,
            reason_hash: ctx.accounts.entry.reason_hash,
            entries: list.entries,
            authority: ctx.accounts.gate_config_authority.key(),
        });
        Ok(())
    }

    /// CG-P5 screen as an instruction, so a caller (auction-engine's
    /// settle-time re-check, AE-P8) can CPI it: succeeds iff `wallet` is
    /// clear on this market, fails [`GateError::Blocklisted`] otherwise,
    /// and fails closed on an unreadable list.
    pub fn check_blocklist(ctx: Context<CheckBlocklist>, _wallet: Pubkey) -> Result<()> {
        blocklist::require_clear(&ctx.accounts.config, &ctx.accounts.entry)
    }

    /// CG-P3: open the lot ledger for `holder` (anyone may pay). The holder
    /// is screened first: a block-listed wallet gets no ledger.
    pub fn open_holder_lots(ctx: Context<OpenHolderLots>, holder: Pubkey) -> Result<()> {
        blocklist::require_clear(&ctx.accounts.config, &ctx.accounts.block_entry)?;
        ctx.accounts.lots.set_inner(HolderLots {
            mint: ctx.accounts.config.mint,
            holder,
            free_qty: 0,
            next_lot_id: 0,
            lot_count: 0,
            lots: [Lot::default(); MAX_LOTS],
            bump: ctx.bumps.lots,
        });
        Ok(())
    }

    /// CG-P3 `append_lot`: record `qty` acquired now, seasoning for the
    /// config's `seasoning_seconds`. Gate-config authority only (it stands
    /// in for auction-engine settle and the primary mint). Re-screens the
    /// holder: block-list precedence on every path (CG-P5).
    pub fn append_lot(
        ctx: Context<AppendLot>,
        qty: u64,
        source: LotSource,
        affiliate: bool,
        exemption_path: u8,
    ) -> Result<()> {
        let cfg = &ctx.accounts.config;
        blocklist::require_clear(cfg, &ctx.accounts.block_entry)?;
        let now = Clock::get()?.unix_timestamp;
        let lots = &mut ctx.accounts.lots;
        let lot = lots.append(
            qty,
            now,
            cfg.seasoning_seconds,
            source,
            affiliate,
            exemption_path,
        )?;
        emit!(LotEvent {
            kind: 0,
            mint: lots.mint,
            owner: lots.holder,
            lot_id: lot.lot_id,
            qty,
            restriction_expiry: lot.restriction_expiry,
            free_qty: lots.free_qty,
        });
        Ok(())
    }

    /// CG-P3 `split_lot`: split `qty` off lot `lot_id`, both halves keeping
    /// the original's seasoning and metadata. Gate-config authority only.
    pub fn split_lot(ctx: Context<SplitLot>, lot_id: u32, qty: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let lots = &mut ctx.accounts.lots;
        let (_, new) = lots.split(lot_id, qty, now)?;
        emit!(LotEvent {
            kind: 1,
            mint: lots.mint,
            owner: lots.holder,
            lot_id: new.lot_id,
            qty,
            restriction_expiry: new.restriction_expiry,
            free_qty: lots.free_qty,
        });
        Ok(())
    }

    /// CG-P1/P6: create the market's eligibility policy. Verifies both
    /// config schemas are SAS schemas under `credential`.
    pub fn initialize_policy(
        ctx: Context<InitializePolicy>,
        credential: Pubkey,
        jurisdictions: Vec<[u8; 2]>,
        min_kyc_level: u8,
        require_accreditation: bool,
    ) -> Result<()> {
        eligibility::check_schema(&ctx.accounts.identity_schema, &credential)?;
        eligibility::check_schema(&ctx.accounts.accreditation_schema, &credential)?;
        let p = &mut ctx.accounts.policy;
        p.mint = ctx.accounts.config.mint;
        p.credential = credential;
        p.set_jurisdictions(&jurisdictions)?;
        p.min_kyc_level = min_kyc_level;
        p.require_accreditation = require_accreditation;
        p.distinct_investors = 0;
        p.bump = ctx.bumps.policy;
        emit!(PolicyUpdated {
            mint: p.mint,
            credential,
            jurisdictions,
            min_kyc_level,
            require_accreditation,
            authority: ctx.accounts.gate_config_authority.key(),
        });
        Ok(())
    }

    /// Governed policy edits. The credential is fixed at init: moving to a
    /// new credential is a new market policy, not an edit.
    pub fn update_policy(
        ctx: Context<UpdatePolicy>,
        jurisdictions: Option<Vec<[u8; 2]>>,
        min_kyc_level: Option<u8>,
        require_accreditation: Option<bool>,
    ) -> Result<()> {
        require!(
            jurisdictions.is_some() || min_kyc_level.is_some() || require_accreditation.is_some(),
            GateError::EmptyPolicyUpdate
        );
        let p = &mut ctx.accounts.policy;
        if let Some(j) = &jurisdictions {
            p.set_jurisdictions(j)?;
        }
        if let Some(k) = min_kyc_level {
            p.min_kyc_level = k;
        }
        if let Some(a) = require_accreditation {
            p.require_accreditation = a;
        }
        emit!(PolicyUpdated {
            mint: p.mint,
            credential: p.credential,
            jurisdictions: p.jurisdictions[..usize::from(p.jurisdiction_count)].to_vec(),
            min_kyc_level: p.min_kyc_level,
            require_accreditation: p.require_accreditation,
            authority: ctx.accounts.gate_config_authority.key(),
        });
        Ok(())
    }

    /// CG-P1 `validate(owner, mint)`: block-list → SAS attestations →
    /// expiry → policy → governor → lot-ledger init, every decision evented
    /// as [`ValidateDecision`]. A rejection changes no state and succeeds
    /// (so the event is kept); an approval writes the holder link that the
    /// Token ACL hooks read.
    pub fn validate(ctx: Context<Validate>, owner: Pubkey) -> Result<()> {
        eligibility::process_validate(ctx, owner)
    }

    /// CG-P4 decrement: the gate-config authority confirms `owner` exited
    /// to zero; its link closes, and its investor leaves the count with its
    /// last wallet.
    pub fn release_holder(ctx: Context<ReleaseHolder>, owner: Pubkey) -> Result<()> {
        let investor = &mut ctx.accounts.investor;
        investor.wallets = investor.wallets.saturating_sub(1);
        let exited = investor.wallets == 0;
        let policy = &mut ctx.accounts.policy;
        if exited {
            policy.distinct_investors = policy.distinct_investors.saturating_sub(1);
            investor.close(ctx.accounts.gate_config_authority.to_account_info())?;
        }
        emit!(HolderReleased {
            mint: policy.mint,
            owner,
            investor_id: ctx.accounts.holder_link.investor_id,
            investor_exited: exited,
            distinct_investors: policy.distinct_investors,
        });
        Ok(())
    }

    /// CG-P4 `project_and_check` (for auction-engine at allocation): fails
    /// [`GateError::GovernorCap`] iff admitting `investor_id` would exceed
    /// the cap. An already-admitted investor always passes.
    pub fn project_and_check(ctx: Context<ProjectAndCheck>, _investor_id: [u8; 16]) -> Result<()> {
        let known = !ctx.accounts.investor.data_is_empty();
        require!(
            known || ctx.accounts.policy.distinct_investors < ctx.accounts.config.holder_cap,
            GateError::GovernorCap
        );
        Ok(())
    }

    /// CG-P1: write the Token ACL extra-account-meta lists for this mint
    /// (`["thaw_extra_account_metas", mint]` and the freeze one).
    pub fn initialize_token_acl_metas(ctx: Context<InitializeTokenAclMetas>) -> Result<()> {
        let a = &ctx.accounts;
        let mint = a.mint.key();
        for (ai, seed, bump, disc) in [
            (
                &a.thaw_metas,
                THAW_EXTRA_METAS_SEED,
                ctx.bumps.thaw_metas,
                CAN_THAW_DISCRIMINATOR,
            ),
            (
                &a.freeze_metas,
                FREEZE_EXTRA_METAS_SEED,
                ctx.bumps.freeze_metas,
                CAN_FREEZE_DISCRIMINATOR,
            ),
        ] {
            eligibility::create_pda(
                &a.gate_config_authority,
                ai,
                &a.system_program,
                EXTRA_METAS_LEN,
                &[seed, mint.as_ref(), &[bump]],
            )?;
            ai.try_borrow_mut_data()?
                .copy_from_slice(&extra_metas_tlv(disc));
        }
        Ok(())
    }

    /// Token ACL `can-thaw-permissionless` hook (sRFC37): succeeds iff the
    /// owner is admitted by the gate right now.
    #[instruction(discriminator = &CAN_THAW_DISCRIMINATOR)]
    pub fn can_thaw_permissionless(ctx: Context<GateHook>) -> Result<()> {
        token_acl::can_thaw(ctx.accounts)
    }

    /// Token ACL `can-freeze-permissionless` hook: succeeds iff the owner is
    /// no longer admitted (revocation).
    #[instruction(discriminator = &CAN_FREEZE_DISCRIMINATOR)]
    pub fn can_freeze_permissionless(ctx: Context<GateHook>) -> Result<()> {
        token_acl::can_freeze(ctx.accounts)
    }

    /// AE-P8 settle-time re-check for one fill (#133, #134): block-list on
    /// both sides, the buyer's admission and SAS attestations re-read, the
    /// seller's seasoned lots debited, the buyer's lot appended. Signed only
    /// by payment-rails' settlement PDA, inside `settle_fill`; any failure
    /// aborts the fill. Returns what the transfer record needs.
    pub fn settle_check(
        ctx: Context<SettleCheck>,
        seller: Pubkey,
        buyer: Pubkey,
        qty: u64,
    ) -> Result<SettleOutcome> {
        settle::process_settle_check(ctx, seller, buyer, qty)
    }

    /// [`settle_check`](credit_gate::settle_check) for the issuer's ROFR
    /// fill (AE-P7): block-list on both sides and the seller's seasoned
    /// lots debited. The issuer is buying its own security back, so it is
    /// not admitted as a holder and gets no lot.
    pub fn settle_check_rofr(
        ctx: Context<SettleCheckRofr>,
        seller: Pubkey,
        issuer: Pubkey,
        qty: u64,
    ) -> Result<SettleOutcome> {
        settle::process_settle_check_rofr(ctx, seller, issuer, qty)
    }

    /// CG-P3 `merge_expired`: permissionless. Folds every seasoned lot into
    /// the holder's FREE bucket. Fails [`GateError::NothingExpired`] when
    /// there is nothing to fold, so every success is a state change.
    pub fn merge_expired(ctx: Context<MergeExpired>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let lots = &mut ctx.accounts.lots;
        let merged = lots.merge_expired(now)?;
        require!(merged > 0, GateError::NothingExpired);
        emit!(LotEvent {
            kind: 2,
            mint: lots.mint,
            owner: lots.holder,
            lot_id: 0,
            qty: merged,
            restriction_expiry: now,
            free_qty: lots.free_qty,
        });
        Ok(())
    }
}

/// [`ConfigUpdated::field_group`] for an `update_config` call: one bit per
/// field group the call supplies (#96).
pub fn update_mask(
    policy_class: bool,
    holder_cap: bool,
    seasoning: bool,
    settlement_horizon: bool,
    attesters: bool,
    blocklist: bool,
) -> u8 {
    [
        (policy_class, field_group::POLICY_CLASS),
        (holder_cap, field_group::HOLDER_CAP),
        (seasoning, field_group::SEASONING),
        (settlement_horizon, field_group::SETTLEMENT_HORIZON),
        (attesters, field_group::ATTESTERS),
        (blocklist, field_group::BLOCKLIST),
    ]
    .iter()
    .filter(|(set, _)| *set)
    .fold(0, |m, (_, bit)| m | bit)
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
pub struct InitializeRoot<'info> {
    /// The singleton root (#93); `init` makes this one-shot.
    #[account(
        init,
        payer = upgrade_authority,
        space = 8 + GateRoot::INIT_SPACE,
        seeds = [GateRoot::SEED],
        bump
    )]
    pub root: Account<'info, GateRoot>,
    /// Program upgrade authority; pays rent. Only it may seed the root.
    #[account(mut)]
    pub upgrade_authority: Signer<'info>,
    /// This program — used only to locate its ProgramData account.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, program::CreditGate>,
    /// Holds the upgrade authority the signer must match. `None` once the
    /// program is finalized, so a finalized program can never be re-rooted.
    #[account(
        constraint = program_data.upgrade_authority_address == Some(upgrade_authority.key())
            @ GateError::NotUpgradeAuthority
    )]
    pub program_data: Account<'info, ProgramData>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct GovernRoot<'info> {
    #[account(
        mut,
        seeds = [GateRoot::SEED],
        bump = root.bump,
        has_one = authority @ GateError::NotRootAuthority
    )]
    pub root: Account<'info, GateRoot>,
    /// Current root authority.
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct AcceptRootAuthority<'info> {
    #[account(
        mut,
        seeds = [GateRoot::SEED],
        bump = root.bump,
        constraint = root.pending_authority != Pubkey::default()
            @ GateError::NoPendingRootAuthority,
        constraint = root.pending_authority == new_authority.key()
            @ GateError::NotProposedAuthority
    )]
    pub root: Account<'info, GateRoot>,
    /// The proposed key; must sign.
    pub new_authority: Signer<'info>,
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
    /// The gate root (#93); its authority is the only allowed signer.
    #[account(
        seeds = [GateRoot::SEED],
        bump = root.bump,
        has_one = authority @ GateError::NotRootAuthority
    )]
    pub root: Account<'info, GateRoot>,
    /// CHECK: mint key is used for PDA seeding; token-type validation of
    /// this mint happens at ACL wiring in P1 (CG-P1).
    pub mint: UncheckedAccount<'info>,
    /// Gate root authority (a single bootstrap key on devnet today, Squads
    /// 2-of-2 is the SRS 08 §8 target); becomes the gate-config authority
    /// and pays rent. Rotate it later with `propose_gate_authority` +
    /// `accept_gate_authority`.
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ProposeGateAuthority<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    /// One pending proposal per mint (`init` fails while one exists).
    #[account(
        init,
        payer = gate_config_authority,
        space = 8 + PendingGateAuthority::INIT_SPACE,
        seeds = [PendingGateAuthority::SEED, mint.key().as_ref()],
        bump
    )]
    pub pending: Account<'info, PendingGateAuthority>,
    /// CHECK: mint key must derive both PDAs above.
    pub mint: UncheckedAccount<'info>,
    /// Current authority (checked by `has_one`); pays the pending rent.
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CancelGateAuthority<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        mut,
        close = proposed_by,
        seeds = [PendingGateAuthority::SEED, mint.key().as_ref()],
        bump = pending.bump,
        has_one = proposed_by
    )]
    pub pending: Account<'info, PendingGateAuthority>,
    /// CHECK: mint key must derive both PDAs above.
    pub mint: UncheckedAccount<'info>,
    pub gate_config_authority: Signer<'info>,
    /// CHECK: rent refund target, pinned by `has_one` on `pending`.
    #[account(mut)]
    pub proposed_by: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct AcceptGateAuthority<'info> {
    #[account(
        mut,
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump
    )]
    pub config: Account<'info, MarketConfig>,
    /// Rent goes to the incoming authority (#96): the outgoing one is
    /// typically the bootstrap key being retired at hand-over, whose
    /// lamports would otherwise be stranded with it.
    #[account(
        mut,
        close = new_authority,
        seeds = [PendingGateAuthority::SEED, mint.key().as_ref()],
        bump = pending.bump,
        has_one = proposed_by,
        constraint = pending.new_authority == new_authority.key() @ GateError::NotProposedAuthority
    )]
    pub pending: Account<'info, PendingGateAuthority>,
    /// CHECK: mint key must derive both PDAs above.
    pub mint: UncheckedAccount<'info>,
    /// The proposed key; must sign. Receives the pending rent.
    #[account(mut)]
    pub new_authority: Signer<'info>,
    /// CHECK: the proposer, pinned by `has_one` on `pending`, so a caller
    /// cannot accept against a proposal it misidentifies.
    pub proposed_by: UncheckedAccount<'info>,
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
    /// Current gate-config authority (checked by `has_one`).
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
    fn update_mask_sets_one_bit_per_supplied_group() {
        use field_group::*;
        assert_eq!(update_mask(false, false, false, false, false, false), 0);
        assert_eq!(
            update_mask(false, true, false, false, false, false),
            HOLDER_CAP
        );
        assert_eq!(
            update_mask(true, false, true, true, false, true),
            POLICY_CLASS | SEASONING | SETTLEMENT_HORIZON | BLOCKLIST
        );
        assert_eq!(update_mask(true, true, true, true, true, true), 0b0011_1111);
        // Every bit is distinct, and INIT/SCHEMAS are never update bits.
        let all = [
            POLICY_CLASS,
            HOLDER_CAP,
            SEASONING,
            SETTLEMENT_HORIZON,
            ATTESTERS,
            BLOCKLIST,
            SCHEMAS,
            INIT,
        ];
        assert_eq!(all.iter().fold(0u8, |m, b| m | b), u8::MAX);
        assert_eq!(all.iter().map(|b| b.count_ones()).sum::<u32>(), 8);
    }

    #[test]
    fn fingerprint_uses_the_full_32_bytes() {
        let h = cfg().fingerprint();
        assert!(h[8..].iter().any(|b| *b != 0), "not a padded 64-bit hash");
    }
}
