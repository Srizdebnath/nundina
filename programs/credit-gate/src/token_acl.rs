//! CG-P1 — the Token ACL (sRFC37) gate interface (#122, #132).
//!
//! Token ACL (`TACLkU6Ci…52TP`) owns a mint's freeze authority through its
//! `MintConfig` PDA. A permissionless thaw (or freeze) of a token account
//! CPIs into this program with the "efficient allow/block list" interface:
//! accounts `[signer, token_account, mint, owner, flag_account,
//! extra_metas, ...extras]`, the extras resolved from the TLV list stored in
//! this program's `["thaw_extra_account_metas", mint]` (resp. freeze) PDA.
//! Returning `Ok` lets Token ACL thaw (freeze); any error stops it.
//!
//! The hooks are read-only (Token ACL passes no payer), so admission state
//! is written beforehand by [`crate::validate`]. At thaw time the gate
//! re-decides everything that can have changed since: block-list,
//! attestation existence, signer, expiry vs settlement horizon, jurisdiction
//! and accreditation policy, and that the attestations are still the ones
//! the admission rests on with the same `investor_id`.
//!
//! `can_freeze_permissionless` is the mirror: anyone may freeze a holder
//! the gate no longer admits (blocked, attestation closed or lapsed, policy
//! changed). Revocation (SRS 08 §5) is therefore enforceable on-chain, not
//! just recorded.
//!
//! Extras (instruction account indices; base accounts are 0–5):
//! 6 config `["config", mint]` · 7 policy `["policy", mint]` · 8 block entry
//! `["blocked", mint, owner]` · 9 holder link `["holder", mint, owner]` ·
//! 10 identity attestation (address read from the link at
//! `IDENTITY_OFFSET`) · 11 accreditation attestation (at
//! `ACCREDITATION_OFFSET`).

use anchor_lang::prelude::*;

use crate::blocklist::BlockEntry;
use crate::eligibility::{evaluate, reject_error, HolderLink, MarketPolicy};
use crate::{GateError, GateReject, MarketConfig};

/// The Token ACL program (devnet and mainnet).
pub const TOKEN_ACL_ID: Pubkey = pubkey!("TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP");
/// Token-2022.
pub const TOKEN_2022_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
/// `sha256("efficient-allow-block-list-standard:can-thaw-permissionless")[..8]`.
pub const CAN_THAW_DISCRIMINATOR: [u8; 8] = [8, 175, 169, 129, 137, 74, 61, 241];
/// `sha256("efficient-allow-block-list-standard:can-freeze-permissionless")[..8]`.
pub const CAN_FREEZE_DISCRIMINATOR: [u8; 8] = [214, 141, 109, 75, 248, 1, 45, 29];
/// PDA seed of the thaw extra-account-meta list (token-acl-interface).
pub const THAW_EXTRA_METAS_SEED: &[u8] = b"thaw_extra_account_metas";
/// PDA seed of the freeze extra-account-meta list.
pub const FREEZE_EXTRA_METAS_SEED: &[u8] = b"freeze_extra_account_metas";
/// Token ACL's per-token-account flag PDA seed: set to 1 only while Token
/// ACL itself is mid-thaw/freeze, so the hooks can tell they are inside it.
pub const FLAG_ACCOUNT_SEED: &[u8] = b"FLAG_ACCOUNT";

const META_LEN: usize = 35;
const EXTRA_COUNT: usize = 6;

/// One `ExtraAccountMeta` (spl-tlv-account-resolution 0.11): discriminator,
/// 32-byte address config, is_signer, is_writable.
fn meta(discriminator: u8, config: &[u8]) -> [u8; META_LEN] {
    let mut m = [0u8; META_LEN];
    m[0] = discriminator;
    m[1..1 + config.len()].copy_from_slice(config);
    m
}

/// Packed seeds: `Literal` = `[1, len, bytes]`, `AccountKey` = `[3, index]`.
fn seeds(literal: &[u8], keys: &[u8]) -> Vec<u8> {
    let mut v = vec![1, literal.len() as u8];
    v.extend_from_slice(literal);
    for k in keys {
        v.extend_from_slice(&[3, *k]);
    }
    v
}

/// The extra-account-meta TLV both hooks use, under `discriminator`:
/// `[8-byte discriminator][u32 length][u32 count][count × 35-byte meta]`.
pub fn extra_metas_tlv(discriminator: [u8; 8]) -> Vec<u8> {
    let metas = [
        meta(1, &seeds(MarketConfig::SEED, &[2])),
        meta(1, &seeds(MarketPolicy::SEED, &[2])),
        meta(1, &seeds(BlockEntry::SEED, &[2, 3])),
        meta(1, &seeds(HolderLink::SEED, &[2, 3])),
        // PubkeyData::AccountData { account_index: 9, data_index }.
        meta(2, &[2, 9, HolderLink::IDENTITY_OFFSET]),
        meta(2, &[2, 9, HolderLink::ACCREDITATION_OFFSET]),
    ];
    let value_len = 4 + EXTRA_COUNT * META_LEN;
    let mut v = Vec::with_capacity(8 + 4 + value_len);
    v.extend_from_slice(&discriminator);
    v.extend_from_slice(&(value_len as u32).to_le_bytes());
    v.extend_from_slice(&(EXTRA_COUNT as u32).to_le_bytes());
    for m in metas {
        v.extend_from_slice(&m);
    }
    v
}

/// Size of each extra-meta account.
pub const EXTRA_METAS_LEN: usize = 8 + 4 + 4 + EXTRA_COUNT * META_LEN;

/// The hooks' common read: is `owner` (holding `token_account`) admitted by
/// the gate right now? `Err` only for unreadable configuration.
fn decide(a: &GateHook) -> Result<core::result::Result<(), GateReject>> {
    // Inside Token ACL's own thaw/freeze only.
    let flag = &a.flag_account;
    let (want_flag, _) = Pubkey::find_program_address(
        &[FLAG_ACCOUNT_SEED, a.token_account.key.as_ref()],
        &TOKEN_ACL_ID,
    );
    require!(
        flag.key() == want_flag
            && flag.owner == &TOKEN_ACL_ID
            && flag.try_borrow_data()?.first() == Some(&1),
        GateError::NotTokenAclFlow
    );
    // The token account is a Token-2022 account of this mint, owned by owner.
    {
        let ta = a.token_account.try_borrow_data()?;
        require!(
            a.token_account.owner == &TOKEN_2022_ID
                && ta.len() >= 64
                && ta[..32] == a.mint.key.to_bytes()
                && ta[32..64] == a.owner.key.to_bytes(),
            GateError::TokenAccountMismatch
        );
    }
    let link = match crate::eligibility::load::<HolderLink>(&a.holder_link)? {
        Some(l) => l,
        None => return Ok(Err(GateReject::AttestationInvalid)),
    };
    if link.identity_attestation != a.identity_attestation.key()
        || link.accreditation_attestation != a.accreditation_attestation.key()
    {
        return Ok(Err(GateReject::AttestationInvalid));
    }
    let now = Clock::get()?.unix_timestamp;
    match evaluate(
        &a.config,
        &a.policy,
        a.owner.key,
        &a.block_entry,
        &a.identity_attestation,
        &a.accreditation_attestation,
        now,
    )? {
        Err(r) => Ok(Err(r)),
        Ok(e) if e.identity.investor_id != link.investor_id => {
            Ok(Err(GateReject::InvestorIdChanged))
        }
        Ok(_) => Ok(Ok(())),
    }
}

/// Token ACL `can-thaw-permissionless`: Ok iff the owner is admitted now.
pub fn can_thaw(a: &GateHook) -> Result<()> {
    match decide(a)? {
        Ok(()) => Ok(()),
        Err(r) => Err(reject_error(r)),
    }
}

/// Token ACL `can-freeze-permissionless`: Ok iff the owner is NOT admitted
/// now (revocation), so a still-eligible holder can't be frozen by anyone.
pub fn can_freeze(a: &GateHook) -> Result<()> {
    match decide(a)? {
        Ok(()) => err!(GateError::StillEligible),
        Err(_) => Ok(()),
    }
}

/// Accounts of both hooks, in the order Token ACL passes them.
#[derive(Accounts)]
pub struct GateHook<'info> {
    /// CHECK: whoever asked Token ACL to thaw/freeze; not used for policy.
    pub signer: UncheckedAccount<'info>,
    /// CHECK: checked in `decide` (Token-2022, mint, owner).
    pub token_account: UncheckedAccount<'info>,
    /// CHECK: the gated mint; derives every PDA below.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: the token account owner (Token ACL checks it against the
    /// account; `decide` re-checks).
    pub owner: UncheckedAccount<'info>,
    /// CHECK: Token ACL's flag PDA; checked in `decide`.
    pub flag_account: UncheckedAccount<'info>,
    /// CHECK: this program's extra-meta list for the hook; Token ACL found
    /// it by its PDA address, so only its existence matters here.
    pub extra_metas: UncheckedAccount<'info>,
    #[account(seeds = [MarketConfig::SEED, mint.key().as_ref()], bump = config.bump)]
    pub config: Account<'info, MarketConfig>,
    #[account(seeds = [MarketPolicy::SEED, mint.key().as_ref()], bump = policy.bump)]
    pub policy: Account<'info, MarketPolicy>,
    /// CHECK: the owner's block entry, pinned by seeds; screened.
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), owner.key().as_ref()], bump)]
    pub block_entry: UncheckedAccount<'info>,
    /// CHECK: the owner's holder link, pinned by seeds; may be empty.
    #[account(seeds = [HolderLink::SEED, mint.key().as_ref(), owner.key().as_ref()], bump)]
    pub holder_link: UncheckedAccount<'info>,
    /// CHECK: SAS identity attestation; verified in `evaluate`.
    pub identity_attestation: UncheckedAccount<'info>,
    /// CHECK: SAS accreditation attestation; verified in `evaluate`.
    pub accreditation_attestation: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct InitializeTokenAclMetas<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    /// CHECK: created here with the thaw TLV; address pinned by seeds.
    #[account(mut, seeds = [THAW_EXTRA_METAS_SEED, mint.key().as_ref()], bump)]
    pub thaw_metas: UncheckedAccount<'info>,
    /// CHECK: created here with the freeze TLV; address pinned by seeds.
    #[account(mut, seeds = [FREEZE_EXTRA_METAS_SEED, mint.key().as_ref()], bump)]
    pub freeze_metas: UncheckedAccount<'info>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discriminators_are_the_interface_hashes() {
        let h = |s: &str| solana_sha256_hasher::hashv(&[s.as_bytes()]).to_bytes();
        assert_eq!(
            CAN_THAW_DISCRIMINATOR[..],
            h("efficient-allow-block-list-standard:can-thaw-permissionless")[..8]
        );
        assert_eq!(
            CAN_FREEZE_DISCRIMINATOR[..],
            h("efficient-allow-block-list-standard:can-freeze-permissionless")[..8]
        );
    }

    #[test]
    fn tlv_layout() {
        let t = extra_metas_tlv(CAN_THAW_DISCRIMINATOR);
        assert_eq!(t.len(), EXTRA_METAS_LEN);
        assert_eq!(t[..8], CAN_THAW_DISCRIMINATOR);
        assert_eq!(
            u32::from_le_bytes(t[8..12].try_into().unwrap()) as usize,
            t.len() - 12
        );
        assert_eq!(u32::from_le_bytes(t[12..16].try_into().unwrap()), 6);
        // The holder link (meta 3) is `["holder", mint(2), owner(3)]`.
        let m3 = &t[16 + 3 * META_LEN..16 + 4 * META_LEN];
        assert_eq!(m3[0], 1);
        assert_eq!(
            &m3[1..13],
            &[1, 6, b'h', b'o', b'l', b'd', b'e', b'r', 3, 2, 3, 3]
        );
        // The identity attestation (meta 4) is read from the link at offset 72.
        let m4 = &t[16 + 4 * META_LEN..16 + 5 * META_LEN];
        assert_eq!(&m4[..4], &[2, 2, 9, 72]);
        assert_eq!(&m4[33..], &[0, 0], "read-only, not a signer");
    }
}
