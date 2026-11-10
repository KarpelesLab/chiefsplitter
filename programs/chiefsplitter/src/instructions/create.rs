use borsh::BorshSerialize;
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
    state::{Splitter, MAX_NAME_LEN, SPLITTER_SEED},
};

/// Create a new fee splitter
///
/// Accounts:
/// 0. `[writable]` Splitter PDA (["splitter", creator, nonce.to_le_bytes()])
/// 1. `[writable, signer]` Creator/payer
/// 2. `[]` System program
pub fn process_create_splitter(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    nonce: u64,
    name: &str,
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let creator_info = next_account_info(account_info_iter)?;
    let system_program_info = next_account_info(account_info_iter)?;

    // Validate creator is signer
    if !creator_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    // Derive and verify splitter PDA
    let nonce_bytes = nonce.to_le_bytes();
    let (expected_pda, bump) = Pubkey::find_program_address(
        &[SPLITTER_SEED, creator_info.key.as_ref(), &nonce_bytes],
        program_id,
    );
    if *splitter_info.key != expected_pda {
        return Err(SplitterError::InvalidPDA.into());
    }

    // Check not already initialized
    if splitter_info.data_len() > 0 {
        return Err(SplitterError::AlreadyInitialized.into());
    }

    // Validate name length
    if name.len() > MAX_NAME_LEN {
        return Err(SplitterError::NameTooLong.into());
    }

    let rent = Rent::get()?;
    let splitter_rent = rent.minimum_balance(Splitter::LEN);

    // Create the splitter account
    let seeds = &[
        SPLITTER_SEED,
        creator_info.key.as_ref(),
        &nonce_bytes,
        &[bump],
    ];

    let current_lamports = splitter_info.lamports();
    if current_lamports == 0 {
        invoke_signed(
            &system_instruction::create_account(
                creator_info.key,
                splitter_info.key,
                splitter_rent,
                Splitter::LEN as u64,
                program_id,
            ),
            &[
                creator_info.clone(),
                splitter_info.clone(),
                system_program_info.clone(),
            ],
            &[seeds],
        )?;
    } else {
        // The PDA address is predictable, so anyone can send lamports to it
        // before creation. `create_account` refuses accounts that already hold
        // lamports, which would permanently block this splitter. Instead, top
        // up to rent exemption if needed, then allocate + assign.
        //
        // Only a pristine system-owned account may be claimed this way (the
        // data_len check above already rejects any account with data).
        if *splitter_info.owner != system_program::id() {
            return Err(SplitterError::InvalidAccountOwner.into());
        }

        let shortfall = splitter_rent.saturating_sub(current_lamports);
        if shortfall > 0 {
            invoke(
                &system_instruction::transfer(creator_info.key, splitter_info.key, shortfall),
                &[
                    creator_info.clone(),
                    splitter_info.clone(),
                    system_program_info.clone(),
                ],
            )?;
        }

        invoke_signed(
            &system_instruction::allocate(splitter_info.key, Splitter::LEN as u64),
            &[splitter_info.clone(), system_program_info.clone()],
            &[seeds],
        )?;

        invoke_signed(
            &system_instruction::assign(splitter_info.key, program_id),
            &[splitter_info.clone(), system_program_info.clone()],
            &[seeds],
        )?;
    }

    // Initialize splitter state
    let splitter = Splitter::new(*creator_info.key, nonce, bump, name.as_bytes());

    let mut data = splitter_info.try_borrow_mut_data()?;
    splitter.serialize(&mut &mut data[..])?;

    msg!("Created splitter PDA {}", splitter_info.key);

    Ok(())
}
