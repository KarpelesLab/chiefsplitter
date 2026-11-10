use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    pubkey::Pubkey,
};

use crate::{error::SplitterError, state::Splitter};

/// Revoke admin (set to Pubkey::default(), irreversible)
///
/// Accounts:
/// 0. `[writable]` Splitter
/// 1. `[signer]` Current admin
pub fn process_revoke_admin(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let admin_info = next_account_info(account_info_iter)?;

    if !admin_info.is_signer {
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
    fn revokes_admin() {
        let program_id = crate::id();
        let admin = Pubkey::new_unique();
        let splitter = Splitter::new(admin, 0, 255, b"t");

        let (res, after) = with_accounts(&splitter, &program_id, &admin, |accounts| {
            process_revoke_admin(&program_id, accounts)
        });
        assert_eq!(res, Ok(()));
        assert!(after.is_admin_revoked());
    }

    #[test]
    fn rejects_splitter_not_owned_by_program() {
        let program_id = crate::id();
        let admin = Pubkey::new_unique();
        let splitter = Splitter::new(admin, 0, 255, b"t");

        let (res, after) = with_accounts(&splitter, &Pubkey::new_unique(), &admin, |accounts| {
            process_revoke_admin(&program_id, accounts)
        });
        assert_eq!(res, Err(wrong_owner()));
        assert_eq!(after.admin, admin);
    }
}
