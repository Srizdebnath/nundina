//! Token CPI helpers shared by every cash/asset path. `transfer_checked`
//! only (decimals-checked), routed through the account's own token program
//! so SPL Token and Token-2022 mints both work.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    close_account, transfer_checked, CloseAccount, Mint, TokenInterface, TransferChecked,
};

/// Move `amount` of `mint` from `from` to `to`. `authority` signs either as a
/// wallet (pass `&[]`) or as a program PDA (pass its seeds). Zero is a
/// no-op so callers can pass fee/reserve legs unconditionally.
pub(crate) fn transfer<'info>(
    token_program: &Interface<'info, TokenInterface>,
    mint: &InterfaceAccount<'info, Mint>,
    from: AccountInfo<'info>,
    to: AccountInfo<'info>,
    authority: AccountInfo<'info>,
    signer_seeds: &[&[&[u8]]],
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    transfer_checked(
        CpiContext::new_with_signer(
            token_program.key(),
            TransferChecked {
                from,
                mint: mint.to_account_info(),
                to,
                authority,
            },
            signer_seeds,
        ),
        amount,
        mint.decimals,
    )
}

/// Close an (emptied) PDA-owned token account, rent to `destination`.
pub(crate) fn close<'info>(
    token_program: &Interface<'info, TokenInterface>,
    account: AccountInfo<'info>,
    destination: AccountInfo<'info>,
    authority: AccountInfo<'info>,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    close_account(CpiContext::new_with_signer(
        token_program.key(),
        CloseAccount {
            account,
            destination,
            authority,
        },
        signer_seeds,
    ))
}
