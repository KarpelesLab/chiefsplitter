use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    pubkey::Pubkey,
};

use crate::{error::SplitterError, state::Splitter};

/// Lock a recipient to a guaranteed minimum share
/// The recipient signs to protect their own rate from being reduced.
///
/// Accounts:
/// 0. `[writable]` Splitter
/// 1. `[signer]` Recipient
pub fn process_lock_recipient(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    min_share: u16,
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let recipient_info = next_account_info(account_info_iter)?;

    if !recipient_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    // Defense in depth: the splitter must be owned by this program
    if splitter_info.owner != program_id {
        return Err(SplitterError::InvalidAccountOwner.into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::program_error::ProgramError;

    /// Run `f` with (splitter, signer) account infos. The splitter account holds
    /// a serialized `splitter` and is owned by `owner`. Returns the result and
    /// the splitter state afterwards.
    fn with_accounts<R>(
        splitter: &Splitter,
        owner: &Pubkey,
        signer: &Pubkey,
        f: impl FnOnce(&[AccountInfo]) -> R,
    ) -> (R, Splitter) {
        let splitter_key = Pubkey::new_unique();
        let system = Pubkey::default();
        let mut data = borsh::to_vec(splitter).unwrap();
        let (mut lamports, mut signer_lamports) = (1_000_000u64, 1_000_000u64);
        let mut signer_data: [u8; 0] = [];
        let result = {
            let accounts = [
                AccountInfo::new(
                    &splitter_key,
                    false,
                    true,
                    &mut lamports,
                    &mut data,
                    owner,
                    false,
                    0,
                ),
                AccountInfo::new(
                    signer,
                    true,
                    false,
                    &mut signer_lamports,
                    &mut signer_data,
                    &system,
                    false,
                    0,
                ),
            ];
            f(&accounts)
        };
        (result, Splitter::try_from_slice(&data).unwrap())
    }

    fn wrong_owner() -> ProgramError {
        SplitterError::InvalidAccountOwner.into()
    }

    #[test]
    fn locks_recipient() {
        let program_id = crate::id();
        let recipient = Pubkey::new_unique();
        let splitter = Splitter::new(recipient, 0, 255, b"t");

        let (res, after) = with_accounts(&splitter, &program_id, &recipient, |accounts| {
            process_lock_recipient(&program_id, accounts, 6000)
        });
        assert_eq!(res, Ok(()));
        assert_eq!(after.recipients[0].locked_share, 6000);
    }

    #[test]
    fn rejects_splitter_not_owned_by_program() {
        let program_id = crate::id();
        let recipient = Pubkey::new_unique();
        let splitter = Splitter::new(recipient, 0, 255, b"t");

        let (res, after) =
            with_accounts(&splitter, &Pubkey::new_unique(), &recipient, |accounts| {
                process_lock_recipient(&program_id, accounts, 6000)
            });
        assert_eq!(res, Err(wrong_owner()));
        assert_eq!(after.recipients[0].locked_share, 0);
    }
}
