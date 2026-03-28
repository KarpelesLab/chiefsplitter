use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program::invoke_signed,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction,
    sysvar::Sysvar,
};

use crate::{
    error::SplitterError,
    state::{SellConfig, Splitter, MAX_APPROVED_PROGRAMS, MAX_WHITELIST, SELL_CONFIG_SEED},
};

/// Create or update sell configuration
///
/// Accounts:
/// 0. `[]` Splitter
/// 1. `[writable]` SellConfig PDA (["sell_config", splitter])
/// 2. `[signer]` Admin
/// 3. `[writable, signer]` Payer (for account creation if needed)
/// 4. `[]` System program
pub fn process_set_sell_config(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    whitelisted_mints: &[Pubkey],
    approved_swap_programs: &[Pubkey],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let sell_config_info = next_account_info(account_info_iter)?;
    let admin_info = next_account_info(account_info_iter)?;
    let payer_info = next_account_info(account_info_iter)?;
    let system_program_info = next_account_info(account_info_iter)?;

    if !admin_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    // Deserialize and validate splitter
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

    // Validate sizes
    if whitelisted_mints.len() > MAX_WHITELIST {
        return Err(SplitterError::TooManyWhitelistedMints.into());
    }
    if approved_swap_programs.len() > MAX_APPROVED_PROGRAMS {
        return Err(SplitterError::TooManyApprovedPrograms.into());
    }

    // Derive and verify sell config PDA
    let (expected_pda, bump) = Pubkey::find_program_address(
        &[SELL_CONFIG_SEED, splitter_info.key.as_ref()],
        program_id,
    );
    if *sell_config_info.key != expected_pda {
        return Err(SplitterError::InvalidPDA.into());
    }

    // Create account if it doesn't exist yet
    if sell_config_info.data_len() == 0 {
        let rent = Rent::get()?;
        let config_rent = rent.minimum_balance(SellConfig::LEN);
        let seeds = &[SELL_CONFIG_SEED, splitter_info.key.as_ref(), &[bump]];

        invoke_signed(
            &system_instruction::create_account(
                payer_info.key,
                sell_config_info.key,
                config_rent,
                SellConfig::LEN as u64,
                program_id,
            ),
            &[
                payer_info.clone(),
                sell_config_info.clone(),
                system_program_info.clone(),
            ],
            &[seeds],
        )?;
    }

    // Build sell config
    let mut config = SellConfig::new(*splitter_info.key, bump);
    config.num_whitelisted = whitelisted_mints.len() as u8;
    for (i, mint) in whitelisted_mints.iter().enumerate() {
        config.whitelisted_mints[i] = *mint;
    }
    config.num_approved_programs = approved_swap_programs.len() as u8;
    for (i, prog) in approved_swap_programs.iter().enumerate() {
        config.approved_programs[i] = *prog;
    }

    // Serialize
    let mut data = sell_config_info.try_borrow_mut_data()?;
    config.serialize(&mut &mut data[..])?;

    msg!(
        "Sell config: {} whitelisted mints, {} approved swap programs",
        whitelisted_mints.len(),
        approved_swap_programs.len()
    );

    Ok(())
}
