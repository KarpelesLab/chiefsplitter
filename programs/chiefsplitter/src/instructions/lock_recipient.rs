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

/// Lock a recipient to a guaranteed minimum share
/// The recipient signs to protect their own rate from being reduced.
///
/// Accounts:
/// 0. `[writable]` Splitter
/// 1. `[signer]` Recipient
pub fn process_lock_recipient(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    min_share: u16,
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let recipient_info = next_account_info(account_info_iter)?;

    if !recipient_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    let mut splitter = {
        let data = splitter_info.try_borrow_data()?;
        Splitter::try_from_slice(&data).map_err(|_| SplitterError::NotInitialized)?
    };

    if !splitter.is_initialized() {
        return Err(SplitterError::NotInitialized.into());
    }

    // Find the recipient in the configured recipients
    let mut found = false;
    for i in 0..splitter.num_recipients as usize {
        if splitter.recipients[i].address == *recipient_info.key {
            let current_share = splitter.recipients[i].share;
            let current_lock = splitter.recipients[i].locked_share;

            // min_share must not exceed current share
            if min_share > current_share {
                return Err(SplitterError::LockShareExceedsCurrent.into());
            }

            // Lock can only increase
            if min_share <= current_lock {
                return Err(SplitterError::LockShareNotIncreased.into());
            }

            splitter.recipients[i].locked_share = min_share;
            found = true;

            msg!(
                "Locked recipient {} to minimum share {} bps",
                recipient_info.key,
                min_share
            );
            break;
        }
    }

    if !found {
        return Err(SplitterError::NotARecipient.into());
    }

    let mut data = splitter_info.try_borrow_mut_data()?;
    splitter.serialize(&mut &mut data[..])?;

    Ok(())
}
