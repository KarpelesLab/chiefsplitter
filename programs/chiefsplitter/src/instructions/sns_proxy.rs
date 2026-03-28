use borsh::BorshDeserialize;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::invoke_signed,
    pubkey::Pubkey,
};

use crate::{
    error::SplitterError,
    state::{Splitter, SNS_PROGRAM_ID, SPLITTER_SEED},
};

/// CPI proxy restricted to Bonfida SNS Name Service program.
/// Allows the admin to set a primary .sol domain, update records, etc.
///
/// Accounts:
/// 0. `[]` Splitter (config + PDA signing)
/// 1. `[signer]` Admin
/// 2. `[]` SNS Name Service program
/// 3..N Remaining accounts forwarded to SNS program
pub fn process_sns_proxy(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    sns_data: &[u8],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let admin_info = next_account_info(account_info_iter)?;
    let sns_program_info = next_account_info(account_info_iter)?;

    // Collect remaining accounts for CPI
    let remaining_accounts: Vec<AccountInfo> = account_info_iter.cloned().collect();

    // Validate admin
    if !admin_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    let splitter = {
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

    // MUST be the Bonfida SNS Name Service program — nothing else
    if *sns_program_info.key != SNS_PROGRAM_ID {
        msg!("SNS proxy only allows CPI to {}", SNS_PROGRAM_ID);
        return Err(SplitterError::InvalidPDA.into());
    }

    // Build CPI instruction from remaining accounts
    let cpi_account_metas: Vec<AccountMeta> = remaining_accounts
        .iter()
        .map(|a| {
            if a.is_writable {
                AccountMeta::new(*a.key, false)
            } else {
                AccountMeta::new_readonly(*a.key, false)
            }
        })
        .collect();

    let sns_ix = Instruction {
        program_id: SNS_PROGRAM_ID,
        accounts: cpi_account_metas,
        data: sns_data.to_vec(),
    };

    // Signer seeds for the splitter PDA
    let nonce_bytes = splitter.nonce.to_le_bytes();
    let bump_slice = [splitter.bump];
    let signer_seeds: &[&[u8]] = &[
        SPLITTER_SEED,
        splitter.creator.as_ref(),
        &nonce_bytes,
        &bump_slice,
    ];

    // Include splitter PDA + all remaining accounts for invoke_signed
    let mut cpi_infos: Vec<AccountInfo> = Vec::with_capacity(2 + remaining_accounts.len());
    cpi_infos.push(splitter_info.clone());
    cpi_infos.push(sns_program_info.clone());
    cpi_infos.extend(remaining_accounts);

    invoke_signed(&sns_ix, &cpi_infos, &[signer_seeds])?;

    msg!("SNS proxy CPI executed");

    Ok(())
}
