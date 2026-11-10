use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    pubkey::Pubkey,
    rent::Rent,
    system_instruction, system_program,
    sysvar::Sysvar,
};

use crate::{
    error::SplitterError,
    instructions::swap_token::is_forbidden_swap_program,
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
    if splitter_info.owner != program_id {
        return Err(SplitterError::InvalidAccountOwner.into());
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

    // Validate sizes
    if whitelisted_mints.len() > MAX_WHITELIST {
        return Err(SplitterError::TooManyWhitelistedMints.into());
    }
    if approved_swap_programs.len() > MAX_APPROVED_PROGRAMS {
        return Err(SplitterError::TooManyApprovedPrograms.into());
    }

    // The swap CPI is signed by the splitter PDA with caller-supplied data, so
    // programs that would let a cranker act directly as the splitter (token
    // programs, system program, ATA program, this program) can never be approved.
    if approved_swap_programs
        .iter()
        .any(|prog| is_forbidden_swap_program(prog, program_id))
    {
        return Err(SplitterError::SwapProgramNotApproved.into());
    }

    // Derive and verify sell config PDA
    let (expected_pda, bump) =
        Pubkey::find_program_address(&[SELL_CONFIG_SEED, splitter_info.key.as_ref()], program_id);
    if *sell_config_info.key != expected_pda {
        return Err(SplitterError::InvalidPDA.into());
    }

    if sell_config_info.data_len() == 0 {
        // Create account if it doesn't exist yet
        if *sell_config_info.owner != system_program::id() {
            return Err(SplitterError::InvalidAccountOwner.into());
        }

        let rent = Rent::get()?;
        let config_rent = rent.minimum_balance(SellConfig::LEN);
        let seeds: &[&[u8]] = &[SELL_CONFIG_SEED, splitter_info.key.as_ref(), &[bump]];

        let current_lamports = sell_config_info.lamports();
        if current_lamports == 0 {
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
        } else {
            // The PDA was pre-funded (anyone can send lamports to it), which
            // makes `create_account` fail. Top up to rent exemption if needed,
            // then allocate + assign with the PDA's signature instead.
            let shortfall = config_rent.saturating_sub(current_lamports);
            if shortfall > 0 {
                invoke(
                    &system_instruction::transfer(payer_info.key, sell_config_info.key, shortfall),
                    &[
                        payer_info.clone(),
                        sell_config_info.clone(),
                        system_program_info.clone(),
                    ],
                )?;
            }
            invoke_signed(
                &system_instruction::allocate(sell_config_info.key, SellConfig::LEN as u64),
                &[sell_config_info.clone(), system_program_info.clone()],
                &[seeds],
            )?;
            invoke_signed(
                &system_instruction::assign(sell_config_info.key, program_id),
                &[sell_config_info.clone(), system_program_info.clone()],
                &[seeds],
            )?;
        }
    } else if sell_config_info.owner != program_id {
        // Updating an existing config: it must be ours
        return Err(SplitterError::InvalidAccountOwner.into());
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
