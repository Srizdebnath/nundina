//! CG-P5 — the market block-list, and the screen every check path runs
//! first (#122).
//!
//! One [`BlockList`] header per mint (`["blocklist", mint]`) and one
//! [`BlockEntry`] PDA per blocked wallet (`["blocked", mint, wallet]`).
//! A wallet is blocked iff its entry account exists. The check is O(1), the
//! list is unbounded, and a caller cannot dodge it: the entry address is
//! re-derived from `(mint, wallet)` on every path, so passing any other
//! account fails the seeds constraint.
//!
//! Precedence (CG-P5): the screen runs **before** anything that grants or
//! extends a position (lot-ledger open, lot append), and a block wins over
//! every allow-side fact. When a market has no list configured
//! (`MarketConfig::blocklist == None`, the pilot default) the screen passes:
//! nothing is blocked. Once a list is configured it can be re-pointed but
//! never cleared (`update_config`). If the config points at anything other
//! than this program's own header for the mint, the screen **fails closed**
//! with [`GateError::BlocklistUnreadable`]: the gate refuses to treat a list
//! it cannot read as empty.
//!
//! Revocation = this list + attestation expiry (SRS 08 §5). The expiry half
//! lands with the SAS read (CG-P6, #52).

use anchor_lang::prelude::*;

use crate::{GateError, GateReject, MarketConfig};

/// Header of one market's block-list. `MarketConfig::blocklist` points here
/// once `initialize_blocklist` has run.
#[account]
#[derive(InitSpace)]
pub struct BlockList {
    /// The gated mint.
    pub mint: Pubkey,
    /// Live entries (wallets currently blocked).
    pub entries: u32,
    /// PDA bump.
    pub bump: u8,
}

impl BlockList {
    /// PDA seed prefix: `["blocklist", mint]`.
    pub const SEED: &'static [u8] = b"blocklist";

    /// The header address for `mint`.
    pub fn address(mint: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[Self::SEED, mint.as_ref()], &crate::ID).0
    }
}

/// One blocked wallet. Existence is the block; closing it unblocks.
#[account]
#[derive(InitSpace)]
pub struct BlockEntry {
    /// The gated mint.
    pub mint: Pubkey,
    /// The blocked wallet.
    pub wallet: Pubkey,
    /// Hash of the off-chain reason (sanctions list hit, court order, …).
    /// Never the reason itself: no PII on-chain.
    pub reason_hash: [u8; 32],
    /// When it was blocked.
    pub blocked_at: i64,
    /// PDA bump.
    pub bump: u8,
}

impl BlockEntry {
    /// PDA seed prefix: `["blocked", mint, wallet]`.
    pub const SEED: &'static [u8] = b"blocked";
}

/// Emitted by `block_wallet` / `unblock_wallet` (CG-P5, CG-P7).
#[event]
#[derive(Debug)]
pub struct BlocklistUpdated {
    /// The gated mint.
    pub mint: Pubkey,
    /// The wallet added or removed.
    pub wallet: Pubkey,
    /// `true` = blocked, `false` = unblocked.
    pub blocked: bool,
    /// Reason hash (the entry's, on unblock).
    pub reason_hash: [u8; 32],
    /// Live entries after this instruction.
    pub entries: u32,
    /// The gate-config authority that signed.
    pub authority: Pubkey,
}

/// The CG-P5 screen for `wallet` on `config`'s market.
///
/// `entry` must be the account at `["blocked", mint, wallet]`; every caller
/// pins that with a seeds constraint. Returns `Some(Blocklisted)` when the
/// entry exists, `None` when the wallet is clear or no list is configured,
/// and an error when the config points at a list this program cannot read.
pub fn screen(config: &MarketConfig, entry: &AccountInfo) -> Result<Option<GateReject>> {
    let Some(list) = config.blocklist else {
        return Ok(None);
    };
    require_keys_eq!(
        list,
        BlockList::address(&config.mint),
        GateError::BlocklistUnreadable
    );
    let blocked = entry.owner == &crate::ID && !entry.data_is_empty();
    Ok(blocked.then_some(GateReject::Blocklisted))
}

/// [`screen`], turning a block into [`GateError::Blocklisted`].
pub fn require_clear(config: &MarketConfig, entry: &AccountInfo) -> Result<()> {
    match screen(config, entry)? {
        None => Ok(()),
        Some(_) => err!(GateError::Blocklisted),
    }
}

#[derive(Accounts)]
pub struct InitializeBlocklist<'info> {
    #[account(
        mut,
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        init,
        payer = gate_config_authority,
        space = 8 + BlockList::INIT_SPACE,
        seeds = [BlockList::SEED, mint.key().as_ref()],
        bump
    )]
    pub blocklist: Account<'info, BlockList>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(wallet: Pubkey)]
pub struct BlockWallet<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized,
        constraint = config.blocklist == Some(blocklist.key()) @ GateError::BlocklistUnreadable
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        mut,
        seeds = [BlockList::SEED, mint.key().as_ref()],
        bump = blocklist.bump
    )]
    pub blocklist: Account<'info, BlockList>,
    /// `init` fails if the wallet is already blocked.
    #[account(
        init,
        payer = gate_config_authority,
        space = 8 + BlockEntry::INIT_SPACE,
        seeds = [BlockEntry::SEED, mint.key().as_ref(), wallet.as_ref()],
        bump
    )]
    pub entry: Account<'info, BlockEntry>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(wallet: Pubkey)]
pub struct UnblockWallet<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = gate_config_authority @ GateError::Unauthorized,
        constraint = config.blocklist == Some(blocklist.key()) @ GateError::BlocklistUnreadable
    )]
    pub config: Account<'info, MarketConfig>,
    #[account(
        mut,
        seeds = [BlockList::SEED, mint.key().as_ref()],
        bump = blocklist.bump
    )]
    pub blocklist: Account<'info, BlockList>,
    /// Closing the entry is the unblock; rent goes to the authority.
    #[account(
        mut,
        close = gate_config_authority,
        seeds = [BlockEntry::SEED, mint.key().as_ref(), wallet.as_ref()],
        bump = entry.bump
    )]
    pub entry: Account<'info, BlockEntry>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
    #[account(mut)]
    pub gate_config_authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(wallet: Pubkey)]
pub struct CheckBlocklist<'info> {
    #[account(
        seeds = [MarketConfig::SEED, mint.key().as_ref()],
        bump = config.bump
    )]
    pub config: Account<'info, MarketConfig>,
    /// CHECK: the block entry address for `(mint, wallet)`, pinned by
    /// seeds; empty when the wallet is not blocked. Read by [`screen`].
    #[account(seeds = [BlockEntry::SEED, mint.key().as_ref(), wallet.as_ref()], bump)]
    pub entry: UncheckedAccount<'info>,
    /// CHECK: mint key must derive the PDAs above.
    pub mint: UncheckedAccount<'info>,
}
