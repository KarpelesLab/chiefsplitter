use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    pubkey::Pubkey,
};

use crate::{
    error::SplitterError,
    state::{Recipient, Splitter, MAX_RECIPIENTS, TOTAL_SHARES},
};

/// Set the distribution configuration for a splitter
///
/// Accounts:
/// 0. `[writable]` Splitter
/// 1. `[signer]` Admin
pub fn process_set_distribution(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    recipients: &[(Pubkey, u16)],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let admin_info = next_account_info(account_info_iter)?;

    // Validate admin is signer
    if !admin_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    // Deserialize splitter
    let mut splitter = {
        let data = splitter_info.try_borrow_data()?;
        Splitter::try_from_slice(&data).map_err(|_| SplitterError::NotInitialized)?
    };

    if !splitter.is_initialized() {
        return Err(SplitterError::NotInitialized.into());
    }

    // Verify admin
    if splitter.is_admin_revoked() {
        return Err(SplitterError::AdminRevoked.into());
    }
    if *admin_info.key != splitter.admin {
        return Err(SplitterError::InvalidAuthority.into());
    }

    // Validate recipient count
    if recipients.len() > MAX_RECIPIENTS {
        return Err(SplitterError::TooManyRecipients.into());
    }

    // Validate total shares = 10000
    let total: u32 = recipients.iter().map(|(_, share)| *share as u32).sum();
    if total != TOTAL_SHARES as u32 {
        return Err(SplitterError::InvalidShareTotal.into());
    }

    // Validate no zero shares
    for (_, share) in recipients {
        if *share == 0 {
            return Err(SplitterError::ZeroShare.into());
        }
    }

    // Check locked share constraints: for any recipient that has a locked_share,
    // the new share must be >= locked_share
    for (addr, new_share) in recipients {
        for i in 0..splitter.num_recipients as usize {
            let existing = &splitter.recipients[i];
            if existing.address == *addr && existing.locked_share > 0 {
                if *new_share < existing.locked_share {
                    msg!(
                        "Recipient {} has locked share {} but new share is {}",
                        addr,
                        existing.locked_share,
                        new_share
                    );
                    return Err(SplitterError::LockedShareViolation.into());
                }
            }
        }
    }

    // Build new recipients array, preserving locked_share for existing recipients
    let mut new_recipients = [Recipient::default(); MAX_RECIPIENTS];
    for (i, (addr, share)) in recipients.iter().enumerate() {
        let mut locked_share = 0u16;
        // Preserve locked_share from existing config
        for j in 0..splitter.num_recipients as usize {
            if splitter.recipients[j].address == *addr {
                locked_share = splitter.recipients[j].locked_share;
                break;
            }
        }
        new_recipients[i] = Recipient {
            address: *addr,
            share: *share,
            locked_share,
        };
    }

    splitter.num_recipients = recipients.len() as u8;
    splitter.recipients = new_recipients;

    // Serialize back
    let mut data = splitter_info.try_borrow_mut_data()?;
    splitter.serialize(&mut &mut data[..])?;

    msg!("Updated distribution with {} recipients", recipients.len());

    Ok(())
}
