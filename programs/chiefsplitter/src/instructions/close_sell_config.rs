use borsh::BorshDeserialize;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    pubkey::Pubkey,
};

use crate::{
    error::SplitterError,
    state::{SellConfig, Splitter, SELL_CONFIG_SEED},
};

/// Close sell configuration (disables selling, returns rent to admin)
///
/// Accounts:
/// 0. `[]` Splitter
/// 1. `[writable]` SellConfig PDA
/// 2. `[writable, signer]` Admin (receives rent refund)
pub fn process_close_sell_config(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let sell_config_info = next_account_info(account_info_iter)?;
    let admin_info = next_account_info(account_info_iter)?;

    if !admin_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    // Validate splitter
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

    // Verify sell config PDA
    let (expected_pda, _) = Pubkey::find_program_address(
        &[SELL_CONFIG_SEED, splitter_info.key.as_ref()],
        program_id,
    );
    if *sell_config_info.key != expected_pda {
        return Err(SplitterError::InvalidPDA.into());
    }

    // Verify sell config is initialized
    {
        let data = sell_config_info.try_borrow_data()?;
        let config =
            SellConfig::try_from_slice(&data).map_err(|_| SplitterError::SellConfigNotFound)?;
        if !config.is_initialized() {
            return Err(SplitterError::SellConfigNotFound.into());
        }
    }

    // Close the account: transfer all lamports to admin, zero out data
    let dest_lamports = admin_info.lamports();
    let source_lamports = sell_config_info.lamports();

    **admin_info.try_borrow_mut_lamports()? = dest_lamports
        .checked_add(source_lamports)
        .ok_or(SplitterError::InvalidPDA)?; // overflow guard
    **sell_config_info.try_borrow_mut_lamports()? = 0;

    // Zero out the data to prevent reuse without re-creation
    let mut data = sell_config_info.try_borrow_mut_data()?;
    data.fill(0);

    msg!("Sell config closed, selling disabled");

    Ok(())
}
