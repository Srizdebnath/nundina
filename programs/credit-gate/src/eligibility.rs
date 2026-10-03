//! CG-P1 / CG-P4 / CG-P6 — who may hold, decided from real SAS
//! attestations (#122, #131, #132).
//!
//! **SAS read (CG-P6).** An attestation counts only if it is an account of
//! the Solana Attestation Service program (`22zoJMtd…4BdG`), carries SAS's
//! attestation discriminator, sits at its canonical PDA
//! `["attestation", credential, schema, nonce]` with `nonce` = the holder
//! wallet, is under this market's credential and the expected schema, was
//! signed by one of `MarketConfig::attesters`, and stays valid for the
//! settlement horizon (`expiry == 0` = SAS "never expires"). Closing the
//! attestation in SAS revokes it: the account stops existing.
//!
//! **Schemas** (SRS 08 §5; byte layouts are SAS's own encoding):
//! - `nundina.identity.v1`: `investor_id: u128` (UUIDv7), `jurisdiction:
//!   String` (ISO 3166-1 alpha-2, exactly 2 bytes), `kyc_level: u8`;
//! - `nundina.accreditation.v1`: `accredited: bool`, `method: u8`.
//!
//! **Order of checks** (CG-P1; the first failure is the reason):
//! block-list (CG-P5 precedence) → identity attestation → accreditation
//! attestation → expiry vs settlement horizon (H4) → jurisdiction / KYC /
//! accreditation policy → (in `validate` only) the holder governor (CG-P4).
//!
//! **Governor (CG-P4).** Holders are counted by `investor_id`, not wallet:
//! one investor's several wallets count once. A new `investor_id` is
//! admitted only while `distinct_investors < holder_cap`; the count drops
//! when the investor's last wallet is released (`release_holder`, signed by
//! the gate-config authority, which confirms the exit to zero).

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::system_program;

use crate::blocklist::{screen, BlockEntry};
use crate::{GateError, GateReject, MarketConfig, MAX_ATTESTERS};

/// The Solana Attestation Service program (same id on devnet and mainnet).
pub const SAS_PROGRAM_ID: Pubkey = pubkey!("22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG");
/// SAS account discriminator of an `Attestation` (`state/discriminator.rs`).
pub const SAS_ATTESTATION_DISCRIMINATOR: u8 = 2;
/// SAS PDA seed of an attestation.
pub const SAS_ATTESTATION_SEED: &[u8] = b"attestation";
/// Allowed-jurisdiction slots in a [`MarketPolicy`].
pub const MAX_JURISDICTIONS: usize = 16;

/// A decoded SAS attestation account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SasAttestation {
    pub nonce: Pubkey,
    pub credential: Pubkey,
    pub schema: Pubkey,
    pub data: Vec<u8>,
    pub signer: Pubkey,
    pub expiry: i64,
    pub token_account: Pubkey,
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if b.len() < n {
        return None;
    }
    let (h, t) = b.split_at(n);
    *b = t;
    Some(h)
}

fn key(b: &mut &[u8]) -> Option<Pubkey> {
    Some(Pubkey::new_from_array(take(b, 32)?.try_into().ok()?))
}

fn u32le(b: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(b, 4)?.try_into().ok()?))
}

impl SasAttestation {
    /// Strict decode of SAS's account bytes: discriminator, then
    /// `nonce | credential | schema | u32-len data | signer | i64 expiry |
    /// token_account`, and nothing after. `None` on any mismatch.
    pub fn parse(bytes: &[u8]) -> Option<SasAttestation> {
        let mut b = bytes;
        if *take(&mut b, 1)?.first()? != SAS_ATTESTATION_DISCRIMINATOR {
            return None;
        }
        let (nonce, credential, schema) = (key(&mut b)?, key(&mut b)?, key(&mut b)?);
        let len = u32le(&mut b)? as usize;
        let data = take(&mut b, len)?.to_vec();
        let signer = key(&mut b)?;
        let expiry = i64::from_le_bytes(take(&mut b, 8)?.try_into().ok()?);
        let token_account = key(&mut b)?;
        b.is_empty().then_some(SasAttestation {
            nonce,
            credential,
            schema,
            data,
            signer,
            expiry,
            token_account,
        })
    }

    /// The canonical SAS address of this attestation.
    pub fn address(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[
                SAS_ATTESTATION_SEED,
                self.credential.as_ref(),
                self.schema.as_ref(),
                self.nonce.as_ref(),
            ],
            &SAS_PROGRAM_ID,
        )
        .0
    }
}

/// `nundina.identity.v1` (SAS layout `[U128, String, U8]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub investor_id: [u8; 16],
    pub jurisdiction: [u8; 2],
    pub kyc_level: u8,
}

impl Identity {
    pub fn parse(data: &[u8]) -> Option<Identity> {
        let mut b = data;
        let investor_id: [u8; 16] = take(&mut b, 16)?.try_into().ok()?;
        if u32le(&mut b)? != 2 {
            return None;
        }
        let jurisdiction: [u8; 2] = take(&mut b, 2)?.try_into().ok()?;
        let kyc_level = *take(&mut b, 1)?.first()?;
        let well_formed = b.is_empty()
            && investor_id != [0; 16]
            && jurisdiction.iter().all(u8::is_ascii_uppercase);
        well_formed.then_some(Identity {
            investor_id,
            jurisdiction,
            kyc_level,
        })
    }
}

/// `nundina.accreditation.v1` (SAS layout `[Bool, U8]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accreditation {
    pub accredited: bool,
    pub method: u8,
}

impl Accreditation {
    pub fn parse(data: &[u8]) -> Option<Accreditation> {
        match data {
            [a @ (0 | 1), method] => Some(Accreditation {
                accredited: *a == 1,
                method: *method,
            }),
            _ => None,
        }
    }
}

/// Per-market eligibility policy and the holder governor's count.
#[account]
#[derive(InitSpace)]
pub struct MarketPolicy {
    /// The gated mint.
    pub mint: Pubkey,
    /// The SAS credential this market accepts attestations under.
    pub credential: Pubkey,
    /// ISO 3166-1 alpha-2 codes allowed to hold; `[..jurisdiction_count]`.
    pub jurisdictions: [[u8; 2]; MAX_JURISDICTIONS],
    /// Live entries in `jurisdictions`.
    pub jurisdiction_count: u8,
    /// Minimum `kyc_level` in the identity attestation.
    pub min_kyc_level: u8,
    /// `accredited == true` required (Rule 506(c) policy classes).
    pub require_accreditation: bool,
    /// Distinct `investor_id`s currently admitted (CG-P4).
    pub distinct_investors: u32,
    /// PDA bump.
    pub bump: u8,
}

impl MarketPolicy {
    /// PDA seed prefix: `["policy", mint]`.
    pub const SEED: &'static [u8] = b"policy";

    pub fn allows(&self, jurisdiction: &[u8; 2]) -> bool {
        self.jurisdictions[..usize::from(self.jurisdiction_count)].contains(jurisdiction)
    }

    pub fn set_jurisdictions(&mut self, list: &[[u8; 2]]) -> Result<()> {
        require!(
            list.len() <= MAX_JURISDICTIONS,
            GateError::TooManyJurisdictions
        );
        for (i, j) in list.iter().enumerate() {
            require!(
                j.iter().all(u8::is_ascii_uppercase) && !list[..i].contains(j),
                GateError::InvalidJurisdiction
            );
        }
        self.jurisdictions = [[0; 2]; MAX_JURISDICTIONS];
        self.jurisdictions[..list.len()].copy_from_slice(list);
        self.jurisdiction_count = list.len() as u8;
        Ok(())
    }
}

/// One admitted wallet (CG-P4: wallet → investor_id). Field order is part
/// of the Token ACL interface: the extra-account metas read the two
/// attestation addresses at fixed offsets ([`HolderLink::IDENTITY_OFFSET`]).
#[account]
#[derive(InitSpace)]
pub struct HolderLink {
    /// The gated mint.
    pub mint: Pubkey,
    /// The holder wallet.
    pub owner: Pubkey,
    /// SAS identity attestation the admission rests on.
    pub identity_attestation: Pubkey,
    /// SAS accreditation attestation the admission rests on.
    pub accreditation_attestation: Pubkey,
    /// From the identity attestation.
    pub investor_id: [u8; 16],
    /// Last successful `validate`.
    pub validated_at: i64,
    /// PDA bump.
    pub bump: u8,
}

impl HolderLink {
    /// PDA seed prefix: `["holder", mint, owner]`.
    pub const SEED: &'static [u8] = b"holder";
    /// Byte offset of `identity_attestation` in the account (after the
    /// 8-byte discriminator, `mint` and `owner`).
    pub const IDENTITY_OFFSET: u8 = 8 + 32 + 32;
    /// Byte offset of `accreditation_attestation`.
    pub const ACCREDITATION_OFFSET: u8 = Self::IDENTITY_OFFSET + 32;
}

/// One admitted investor and how many of its wallets are admitted.
#[account]
#[derive(InitSpace)]
pub struct InvestorRecord {
    pub mint: Pubkey,
    pub investor_id: [u8; 16],
    pub wallets: u32,
    pub bump: u8,
}

impl InvestorRecord {
    /// PDA seed prefix: `["investor", mint, investor_id]`.
    pub const SEED: &'static [u8] = b"investor";
}

/// Which SAS attestation a decision rested on (#131): who signed it, under
/// which schema, and until when (`0` = SAS "never expires"). No PII: the
/// attestation's data stays behind `attestation_inputs_hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, AnchorSerialize, AnchorDeserialize)]
pub struct AttestationRef {
    /// The SAS attestation account.
    pub attestation: Pubkey,
    /// The attester that signed it (one of `MarketConfig::attesters`).
    pub attester: Pubkey,
    /// Its SAS schema.
    pub schema: Pubkey,
    /// Its SAS expiry (unix seconds).
    pub expiry: i64,
}

impl AttestationRef {
    fn of(ai: &AccountInfo, att: &SasAttestation) -> AttestationRef {
        AttestationRef {
            attestation: *ai.key,
            attester: att.signer,
            schema: att.schema,
            expiry: att.expiry,
        }
    }
}

/// Emitted on every `validate` decision (CG-P1, CG-P7).
#[event]
#[derive(Debug)]
pub struct ValidateDecision {
    /// Rejection reason; `None` = approved.
    pub reject: Option<GateReject>,
    /// The gated mint.
    pub mint: Pubkey,
    /// Owner whose eligibility was decided.
    pub owner: Pubkey,
    /// SHA-256 over both attestation addresses and data (no PII on-chain).
    pub attestation_inputs_hash: [u8; 32],
    /// The identity attestation the approval rests on (#131). All zero on
    /// a rejection: `reject` says why, and nothing was admitted.
    pub identity: AttestationRef,
    /// The accreditation attestation read; zero as for `identity`.
    pub accreditation: AttestationRef,
    /// Distinct investors admitted after this decision.
    pub distinct_investors: u32,
}

/// Emitted by `initialize_policy` / `update_policy`.
#[event]
#[derive(Debug)]
pub struct PolicyUpdated {
    pub mint: Pubkey,
    pub credential: Pubkey,
    pub jurisdictions: Vec<[u8; 2]>,
    pub min_kyc_level: u8,
    pub require_accreditation: bool,
    pub authority: Pubkey,
}

/// Emitted when a wallet is released (CG-P4 decrement).
#[event]
#[derive(Debug)]
pub struct HolderReleased {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub investor_id: [u8; 16],
    /// The investor's last wallet went, so it left the count.
    pub investor_exited: bool,
    pub distinct_investors: u32,
}

/// What an eligible holder's attestations say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Eligible {
    pub identity: Identity,
    pub accreditation: Accreditation,
    pub inputs_hash: [u8; 32],
    /// The identity attestation it rests on (#131).
    pub identity_ref: AttestationRef,
    /// The accreditation attestation it rests on.
    pub accreditation_ref: AttestationRef,
}

fn read_attestation(
    ai: &AccountInfo,
    owner: &Pubkey,
    schema: &Pubkey,
    config: &MarketConfig,
    policy: &MarketPolicy,
    now: i64,
) -> core::result::Result<SasAttestation, GateReject> {
    if ai.owner != &SAS_PROGRAM_ID {
        return Err(GateReject::AttestationInvalid);
    }
    let data = ai
        .try_borrow_data()
        .map_err(|_| GateReject::AttestationInvalid)?;
    let att = SasAttestation::parse(&data).ok_or(GateReject::AttestationInvalid)?;
    let attesters = &config.attesters[..usize::from(config.attester_count).min(MAX_ATTESTERS)];
    let sound = att.nonce == *owner
        && att.schema == *schema
        && att.credential == policy.credential
        && attesters.contains(&att.signer)
        && att.address() == *ai.key;
    if !sound {
        return Err(GateReject::AttestationInvalid);
    }
    // H4: valid for the whole settlement window, not just at this instant.
    let horizon = now.saturating_add(config.settlement_horizon_seconds);
    if att.expiry != 0 && att.expiry < horizon {
        return Err(GateReject::AttestationExpiry);
    }
    Ok(att)
}

/// The CG-P1 decision for `owner`, minus the governor. `Err` only for a
/// configuration the gate cannot read (fail closed); `Ok(Err(reason))` for
/// a typed rejection.
pub fn evaluate(
    config: &MarketConfig,
    policy: &MarketPolicy,
    owner: &Pubkey,
    block_entry: &AccountInfo,
    identity_ai: &AccountInfo,
    accreditation_ai: &AccountInfo,
    now: i64,
) -> Result<core::result::Result<Eligible, GateReject>> {
    if let Some(r) = screen(config, block_entry)? {
        return Ok(Err(r));
    }
    let id = match read_attestation(
        identity_ai,
        owner,
        &config.identity_schema,
        config,
        policy,
        now,
    ) {
        Ok(a) => a,
        Err(r) => return Ok(Err(r)),
    };
    let acc = match read_attestation(
        accreditation_ai,
        owner,
        &config.accreditation_schema,
        config,
        policy,
        now,
    ) {
        Ok(a) => a,
        Err(r) => return Ok(Err(r)),
    };
    let Some(identity) = Identity::parse(&id.data) else {
        return Ok(Err(GateReject::AttestationInvalid));
    };
    let Some(accreditation) = Accreditation::parse(&acc.data) else {
        return Ok(Err(GateReject::AttestationInvalid));
    };
    if !policy.allows(&identity.jurisdiction) {
        return Ok(Err(GateReject::JurisdictionRejected));
    }
    if identity.kyc_level < policy.min_kyc_level {
        return Ok(Err(GateReject::KycLevelTooLow));
    }
    if policy.require_accreditation && !accreditation.accredited {
        return Ok(Err(GateReject::NotAccredited));
    }
    let inputs_hash = solana_sha256_hasher::hashv(&[
        identity_ai.key.as_ref(),
        accreditation_ai.key.as_ref(),
        &id.data,
        &acc.data,
    ])
    .to_bytes();
    Ok(Ok(Eligible {
        identity,
        accreditation,
        inputs_hash,
        identity_ref: AttestationRef::of(identity_ai, &id),
        accreditation_ref: AttestationRef::of(accreditation_ai, &acc),
    }))
}

/// [`GateReject`] → the error a Token ACL hook or a CPI caller sees.
pub fn reject_error(r: GateReject) -> Error {
    match r {
        GateReject::Blocklisted => error!(GateError::Blocklisted),
        GateReject::AttestationInvalid => error!(GateError::AttestationInvalid),
        GateReject::AttestationExpiry => error!(GateError::AttestationExpired),
        GateReject::JurisdictionRejected => error!(GateError::JurisdictionRejected),
        GateReject::GovernorCap => error!(GateError::GovernorCap),
        GateReject::ConfigInvalid => error!(GateError::ConfigFieldMissing),
        GateReject::LotLedgerFull => error!(GateError::LotLedgerFull),
        GateReject::NotAccredited => error!(GateError::NotAccredited),
        GateReject::KycLevelTooLow => error!(GateError::KycLevelTooLow),
        GateReject::InvestorIdChanged => error!(GateError::InvestorIdChanged),
    }
}

/// Create a program-owned PDA at `target`, even if someone pre-funded it
/// with lamports (a plain `create_account` would fail and block the holder).
pub fn create_pda<'info>(
    payer: &AccountInfo<'info>,
    target: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    space: usize,
    seeds: &[&[u8]],
) -> Result<()> {
    let rent = Rent::get()?.minimum_balance(space);
    let have = target.lamports();
    if have < rent {
        system_program::transfer(
            CpiContext::new(
                system_program::ID,
                system_program::Transfer {
                    from: payer.clone(),
                    to: target.clone(),
                },
            ),
            rent - have,
        )?;
    }
    let signer: &[&[&[u8]]] = &[seeds];
    invoke_signed(
        &anchor_lang::solana_program::system_instruction::allocate(target.key, space as u64),
        &[target.clone(), system.clone()],
        signer,
    )?;
    invoke_signed(
        &anchor_lang::solana_program::system_instruction::assign(target.key, &crate::ID),
        &[target.clone(), system.clone()],
        signer,
    )?;
    Ok(())
}

/// Serialize an `#[account]` value (discriminator + borsh) into `ai`.
pub fn store<T: AccountSerialize>(ai: &AccountInfo, value: &T) -> Result<()> {
    let mut data = ai.try_borrow_mut_data()?;
    let mut w: &mut [u8] = &mut data;
    value.try_serialize(&mut w)
}

/// Read an `#[account]` value this program owns; `None` if `ai` is empty.
pub fn load<T: AccountDeserialize>(ai: &AccountInfo) -> Result<Option<T>> {
    if ai.data_is_empty() {
        return Ok(None);
    }
    require_keys_eq!(*ai.owner, crate::ID, GateError::ConfigFieldMissing);
    let data = ai.try_borrow_data()?;
    Ok(Some(T::try_deserialize(&mut &data[..])?))
}

/// `validate` body (see the instruction doc in `lib.rs`).
pub fn process_validate(ctx: Context<Validate>, owner: Pubkey) -> Result<()> {
    let a = ctx.accounts;
    let mint = a.mint.key();
    let now = Clock::get()?.unix_timestamp;
    let decided = evaluate(
        &a.config,
        &a.policy,
        &owner,
        &a.block_entry,
        &a.identity_attestation,
        &a.accreditation_attestation,
        now,
    )?;
    let (outcome, hash, refs) = match decided {
        Err(r) => (Err(r), [0; 32], Default::default()),
        Ok(e) => (
            admit(a, &ctx.bumps, owner, mint, &e, now)?,
            e.inputs_hash,
            (e.identity_ref, e.accreditation_ref),
        ),
    };
    emit!(ValidateDecision {
        reject: outcome.err(),
        mint,
        owner,
        attestation_inputs_hash: hash,
        identity: refs.0,
        accreditation: refs.1,
        distinct_investors: a.policy.distinct_investors,
    });
    Ok(())
}

/// The governor and the writes of an approval. `Ok(Err(r))` is a typed
/// rejection that changed nothing; `Err` aborts the transaction.
fn admit(
    a: &mut Validate,
    bumps: &ValidateBumps,
    owner: Pubkey,
    mint: Pubkey,
    e: &Eligible,
    now: i64,
) -> Result<core::result::Result<(), GateReject>> {
    let id = e.identity.investor_id;
    let (investor_key, investor_bump) =
        Pubkey::find_program_address(&[InvestorRecord::SEED, mint.as_ref(), &id], &crate::ID);
    require_keys_eq!(
        a.investor.key(),
        investor_key,
        GateError::InvestorRecordMismatch
    );
    let payer = a.payer.to_account_info();
    let system = a.system_program.to_account_info();
    let link_ai = a.holder_link.to_account_info();

    if let Some(mut link) = load::<HolderLink>(&link_ai)? {
        // Re-validation: same investor only; refresh the attestations it rests on.
        if link.investor_id != id {
            return Ok(Err(GateReject::InvestorIdChanged));
        }
        link.identity_attestation = a.identity_attestation.key();
        link.accreditation_attestation = a.accreditation_attestation.key();
        link.validated_at = now;
        store(&link_ai, &link)?;
        return Ok(Ok(()));
    }

    // A new wallet. Governor first: a new investor_id needs headroom.
    let investor_ai = a.investor.to_account_info();
    match load::<InvestorRecord>(&investor_ai)? {
        Some(mut inv) => {
            inv.wallets = inv.wallets.checked_add(1).ok_or(GateError::MathOverflow)?;
            store(&investor_ai, &inv)?;
        }
        None => {
            if a.policy.distinct_investors >= a.config.holder_cap {
                return Ok(Err(GateReject::GovernorCap));
            }
            create_pda(
                &payer,
                &investor_ai,
                &system,
                8 + InvestorRecord::INIT_SPACE,
                &[InvestorRecord::SEED, mint.as_ref(), &id, &[investor_bump]],
            )?;
            store(
                &investor_ai,
                &InvestorRecord {
                    mint,
                    investor_id: id,
                    wallets: 1,
                    bump: investor_bump,
                },
            )?;
            a.policy.distinct_investors += 1;
        }
    }
    create_pda(
        &payer,
        &link_ai,
        &system,
        8 + HolderLink::INIT_SPACE,
        &[
            HolderLink::SEED,
            mint.as_ref(),
            owner.as_ref(),
            &[bumps.holder_link],
        ],
    )?;
    store(
        &link_ai,
        &HolderLink {
            mint,
            owner,
            identity_attestation: a.identity_attestation.key(),
            accreditation_attestation: a.accreditation_attestation.key(),
            investor_id: id,
            validated_at: now,
            bump: bumps.holder_link,
        },
    )?;
    // CG-P1 step 5: the holder's lot ledger exists before any lot arrives.
    let lots_ai = a.lots.to_account_info();
    if lots_ai.data_is_empty() {
        create_pda(
            &payer,
            &lots_ai,
            &system,
            8 + crate::HolderLots::INIT_SPACE,
            &[
                crate::HolderLots::SEED,
                mint.as_ref(),
                owner.as_ref(),
                &[bumps.lots],
            ],
        )?;
        store(
            &lots_ai,
            &crate::HolderLots {
                mint,
                holder: owner,
                free_qty: 0,
                next_lot_id: 0,
                lot_count: 0,
                lots: [crate::Lot::default(); crate::MAX_LOTS],
                bump: bumps.lots,
            },
        )?;
    }
    Ok(Ok(()))
}

#[derive(Accounts)]
pub struct InitializePolicy<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        init,
        payer = gate_config_authority,
        space = 8 + MarketPolicy::INIT_SPACE,
        seeds = [MarketPolicy::SEED, mint.key().as_ref()],
        bump
    )]
    pub policy: Account<'info, MarketPolicy>,
    /// CHECK: must be the config's `identity_schema`, a SAS schema under
    /// `credential` (checked by [`check_schema`]).
    #[account(address = config.identity_schema @ GateError::InvalidSchema)]
    pub identity_schema: UncheckedAccount<'info>,
    /// CHECK: as above, for `accreditation_schema`.
    #[account(address = config.accreditation_schema @ GateError::InvalidSchema)]
    pub accreditation_schema: UncheckedAccount<'info>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// A SAS `Schema` account (discriminator 1) whose credential is `credential`.
pub fn check_schema(ai: &AccountInfo, credential: &Pubkey) -> Result<()> {
    let data = ai.try_borrow_data()?;
    require!(
        ai.owner == &SAS_PROGRAM_ID
            && data.len() > 33
            && data[0] == 1
            && data[1..33] == credential.to_bytes(),
        GateError::InvalidSchema
    );
    Ok(())
}

#[derive(Accounts)]
pub struct UpdatePolicy<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        mut,
        seeds = [MarketPolicy::SEED, mint.key().as_ref()],
        bump = policy.bump
    )]
    pub policy: Account<'info, MarketPolicy>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    pub gate_config_authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(owner: Pubkey)]
pub struct Validate<'info> {
    #[account(seeds = [MarketConfig::SEED, mint.key().as_ref()], bump = config.bump)]
    pub config: Account<'info, MarketConfig>,
    #[account(mut, seeds = [MarketPolicy::SEED, mint.key().as_ref()], bump = policy.bump)]
    pub policy: Account<'info, MarketPolicy>,
    /// CHECK: the owner's block entry, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), owner.as_ref()], bump)]
    pub block_entry: UncheckedAccount<'info>,
    /// CHECK: SAS identity attestation; verified in `evaluate`.
    pub identity_attestation: UncheckedAccount<'info>,
    /// CHECK: SAS accreditation attestation; verified in `evaluate`.
    pub accreditation_attestation: UncheckedAccount<'info>,
    /// CHECK: the holder link PDA, pinned by seeds; created on approval.
    #[account(mut, seeds = [HolderLink::SEED, mint.key().as_ref(), owner.as_ref()], bump)]
    pub holder_link: UncheckedAccount<'info>,
    /// CHECK: `["investor", mint, investor_id]`; the id comes from the
    /// identity attestation, so the address is checked in the handler.
    #[account(mut)]
    pub investor: UncheckedAccount<'info>,
    /// CHECK: the owner's lot ledger PDA, pinned by seeds; opened on
    /// approval if absent (CG-P1 step 5).
    #[account(mut, seeds = [crate::HolderLots::SEED, mint.key().as_ref(), owner.as_ref()], bump)]
    pub lots: UncheckedAccount<'info>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(owner: Pubkey)]
pub struct ReleaseHolder<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(mut, seeds = [MarketPolicy::SEED, mint.key().as_ref()], bump = policy.bump)]
    pub policy: Account<'info, MarketPolicy>,
    #[account(
        mut,
        close = gate_config_authority,
        seeds = [HolderLink::SEED, mint.key().as_ref(), owner.as_ref()],
        bump = holder_link.bump
    )]
    pub holder_link: Account<'info, HolderLink>,
    #[account(
        mut,
        seeds = [InvestorRecord::SEED, mint.key().as_ref(), holder_link.investor_id.as_ref()],
        bump = investor.bump
    )]
    pub investor: Account<'info, InvestorRecord>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(investor_id: [u8; 16])]
pub struct ProjectAndCheck<'info> {
    #[account(seeds = [MarketConfig::SEED, mint.key().as_ref()], bump = config.bump)]
    pub config: Account<'info, MarketConfig>,
    #[account(seeds = [MarketPolicy::SEED, mint.key().as_ref()], bump = policy.bump)]
    pub policy: Account<'info, MarketPolicy>,
    /// CHECK: the investor record address for `investor_id`; empty when
    /// the investor is new.
    #[account(seeds = [InvestorRecord::SEED, mint.key().as_ref(), investor_id.as_ref()], bump)]
    pub investor: UncheckedAccount<'info>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att_bytes(a: &SasAttestation) -> Vec<u8> {
        let mut v = vec![SAS_ATTESTATION_DISCRIMINATOR];
        for k in [a.nonce, a.credential, a.schema] {
            v.extend_from_slice(k.as_ref());
        }
        v.extend_from_slice(&(a.data.len() as u32).to_le_bytes());
        v.extend_from_slice(&a.data);
        v.extend_from_slice(a.signer.as_ref());
        v.extend_from_slice(&a.expiry.to_le_bytes());
        v.extend_from_slice(a.token_account.as_ref());
        v
    }

    fn sample() -> SasAttestation {
        SasAttestation {
            nonce: Pubkey::new_from_array([1; 32]),
            credential: Pubkey::new_from_array([2; 32]),
            schema: Pubkey::new_from_array([3; 32]),
            data: vec![1, 7],
            signer: Pubkey::new_from_array([4; 32]),
            expiry: 1_700_000_000,
            token_account: Pubkey::default(),
        }
    }

    #[test]
    fn sas_attestation_round_trips_and_rejects_any_damage() {
        let a = sample();
        let b = att_bytes(&a);
        assert_eq!(SasAttestation::parse(&b), Some(a.clone()));
        // Wrong discriminator (a SAS credential or schema account).
        let mut wrong = b.clone();
        wrong[0] = 1;
        assert_eq!(SasAttestation::parse(&wrong), None);
        // Truncated, trailing bytes, lying length prefix.
        assert_eq!(SasAttestation::parse(&b[..b.len() - 1]), None);
        let mut long = b.clone();
        long.push(0);
        assert_eq!(SasAttestation::parse(&long), None);
        let mut lying = b.clone();
        lying[97] = 200;
        assert_eq!(SasAttestation::parse(&lying), None);
        assert_eq!(SasAttestation::parse(&[]), None);
    }

    #[test]
    fn identity_schema_is_strict() {
        let mut d = vec![9u8; 16];
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(b"US");
        d.push(2);
        assert_eq!(
            Identity::parse(&d),
            Some(Identity {
                investor_id: [9; 16],
                jurisdiction: *b"US",
                kyc_level: 2
            })
        );
        let mut lower = d.clone();
        lower[20..22].copy_from_slice(b"us");
        assert_eq!(
            Identity::parse(&lower),
            None,
            "codes are upper-case alpha-2"
        );
        let mut zero_id = d.clone();
        zero_id[..16].fill(0);
        assert_eq!(
            Identity::parse(&zero_id),
            None,
            "a zero investor_id is no id"
        );
        let mut three = vec![9u8; 16];
        three.extend_from_slice(&3u32.to_le_bytes());
        three.extend_from_slice(b"USA");
        three.push(2);
        assert_eq!(Identity::parse(&three), None);
        assert_eq!(Identity::parse(&d[..d.len() - 1]), None);
    }

    #[test]
    fn accreditation_schema_is_strict() {
        assert_eq!(
            Accreditation::parse(&[1, 3]),
            Some(Accreditation {
                accredited: true,
                method: 3
            })
        );
        assert_eq!(
            Accreditation::parse(&[0, 0]).map(|a| a.accredited),
            Some(false)
        );
        assert_eq!(Accreditation::parse(&[2, 0]), None, "SAS bools are 0/1");
        assert_eq!(Accreditation::parse(&[1]), None);
        assert_eq!(Accreditation::parse(&[1, 0, 0]), None);
    }

    #[test]
    fn jurisdictions_are_validated_and_replaced_whole() {
        let mut p = MarketPolicy {
            mint: Pubkey::default(),
            credential: Pubkey::default(),
            jurisdictions: [[0; 2]; MAX_JURISDICTIONS],
            jurisdiction_count: 0,
            min_kyc_level: 0,
            require_accreditation: true,
            distinct_investors: 0,
            bump: 0,
        };
        p.set_jurisdictions(&[*b"US", *b"GB"]).unwrap();
        assert!(p.allows(b"US") && p.allows(b"GB") && !p.allows(b"IN"));
        p.set_jurisdictions(&[*b"IN"]).unwrap();
        assert!(!p.allows(b"US") && p.allows(b"IN"));
        assert!(!p.allows(&[0, 0]), "empty slots never match");
        assert!(p.set_jurisdictions(&[*b"us"]).is_err());
        assert!(p.set_jurisdictions(&[*b"US", *b"US"]).is_err());
        assert!(p
            .set_jurisdictions(&[*b"AA"; MAX_JURISDICTIONS + 1])
            .is_err());
    }
}
