use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    pubkey::Pubkey,
};

use crate::{
    error::SplitterError,
    state::{Splitter, MAX_NAME_LEN},
};

/// Update the splitter's name (admin only)
///
/// Accounts:
/// 0. `[writable]` Splitter
/// 1. `[signer]` Admin
pub fn process_set_splitter_name(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    name: &str,
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let admin_info = next_account_info(account_info_iter)?;

    if !admin_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    if name.len() > MAX_NAME_LEN {
        return Err(SplitterError::NameTooLong.into());
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

    splitter.set_name(name.as_bytes());

    let mut data = splitter_info.try_borrow_mut_data()?;
    splitter.serialize(&mut &mut data[..])?;

    msg!("Splitter name updated");

    Ok(())
}
