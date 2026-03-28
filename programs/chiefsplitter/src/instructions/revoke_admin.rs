use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    pubkey::Pubkey,
};

use crate::{
    error::SplitterError,
    state::Splitter,
};

/// Revoke admin (set to Pubkey::default(), irreversible)
///
/// Accounts:
/// 0. `[writable]` Splitter
/// 1. `[signer]` Current admin
pub fn process_revoke_admin(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let admin_info = next_account_info(account_info_iter)?;

    if !admin_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    let mut splitter = {
        let data = splitter_info.try_borrow_data()?;
        Splitter::try_from_slice(&data).map_err(|_| SplitterError::NotInitialized)?
    };

    if !splitter.is_initialized() {
        return Err(SplitterError::NotInitialized.into());
    }

    if splitter.is_admin_revoked() {
        return Err(SplitterError::AdminRevoked.into());
    }

    if *admin_info.key != splitter.admin {
        return Err(SplitterError::InvalidAuthority.into());
    }

    splitter.admin = Pubkey::default();

    let mut data = splitter_info.try_borrow_mut_data()?;
    splitter.serialize(&mut &mut data[..])?;

    msg!("Admin revoked (irreversible)");

    Ok(())
}
