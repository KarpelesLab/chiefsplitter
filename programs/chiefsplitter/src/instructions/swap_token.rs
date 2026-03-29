use borsh::BorshDeserialize;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::invoke_signed,
    program_pack::Pack,
    pubkey::Pubkey,
};

use crate::{
    error::SplitterError,
    state::{SellConfig, Splitter, SELL_CONFIG_SEED, SPLITTER_SEED},
};

/// Read the token balance from an SPL token account's raw data.
fn read_token_balance(account: &AccountInfo) -> Result<u64, solana_program::program_error::ProgramError> {
    let data = account.try_borrow_data()?;
    let acct = spl_token_2022::extension::StateWithExtensions::<spl_token_2022::state::Account>::unpack(&data)?;
    Ok(acct.base.amount)
}

/// Swap non-whitelisted tokens via an admin-approved DEX (permissionless crank).
///
/// The cranker builds the swap instruction off-chain (e.g. via Jupiter SDK)
/// and passes the raw data + all required accounts. Our program:
///   1. Validates the sell config, source mint, dest mint, and swap program
///   2. Records balances before
///   3. CPI to the swap program (invoke_signed — PDA is the authority)
///   4. Verifies source decreased and dest increased
///   5. If dest is native mint (wSOL): closes the account to unwrap to SOL
///
/// Accounts:
/// 0. `[writable]` Splitter PDA
/// 1. `[]` SellConfig PDA
/// 2. `[]` Source token mint
/// 3. `[writable]` Splitter's source token account
/// 4. `[writable]` Splitter's destination token account
/// 5. `[]` Destination token mint
/// 6. `[]` Swap program (must be in approved list)
/// 7. `[]` Token program (for wSOL close_account if needed)
/// 8..N Remaining accounts forwarded to swap program
pub fn process_swap_token(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    swap_data: &[u8],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let sell_config_info = next_account_info(account_info_iter)?;
    let source_mint_info = next_account_info(account_info_iter)?;
    let source_token_info = next_account_info(account_info_iter)?;
    let dest_token_info = next_account_info(account_info_iter)?;
    let dest_mint_info = next_account_info(account_info_iter)?;
    let swap_program_info = next_account_info(account_info_iter)?;
    let token_program_info = next_account_info(account_info_iter)?;

    // Collect remaining accounts for CPI pass-through
    let remaining_accounts: Vec<AccountInfo> = account_info_iter.cloned().collect();

    // --- Validate splitter ---
    let splitter = {
        let data = splitter_info.try_borrow_data()?;
        Splitter::try_from_slice(&data).map_err(|_| SplitterError::NotInitialized)?
    };
    if !splitter.is_initialized() {
        return Err(SplitterError::NotInitialized.into());
    }

    // --- Validate sell config ---
    let (expected_config_pda, _) = Pubkey::find_program_address(
        &[SELL_CONFIG_SEED, splitter_info.key.as_ref()],
        program_id,
    );
    if *sell_config_info.key != expected_config_pda {
        return Err(SplitterError::InvalidPDA.into());
    }
    let sell_config = {
        let data = sell_config_info.try_borrow_data()?;
        SellConfig::try_from_slice(&data).map_err(|_| SplitterError::SellConfigNotFound)?
    };
    if !sell_config.is_initialized() {
        return Err(SplitterError::SellConfigNotFound.into());
    }
    if sell_config.splitter != *splitter_info.key {
        return Err(SplitterError::SellConfigSplitterMismatch.into());
    }

    // --- Source mint must NOT be whitelisted ---
    if sell_config.is_whitelisted(source_mint_info.key) {
        return Err(SplitterError::TokenWhitelisted.into());
    }

    // --- Dest mint must be native (wSOL → auto-unwrap) OR whitelisted ---
    // SPL Token native mint (So11111111111111111111111111111111111111112)
    const SPL_NATIVE_MINT: Pubkey = Pubkey::new_from_array([
        0x06, 0x9b, 0x88, 0x57, 0xfe, 0xab, 0x81, 0x84,
        0xfb, 0x68, 0x7f, 0x63, 0x46, 0x18, 0xc0, 0x35,
        0xda, 0xc4, 0x39, 0xdc, 0x1a, 0xeb, 0x3b, 0x55,
        0x98, 0xa0, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x01,
    ]);
    let is_native_dest = *dest_mint_info.key == SPL_NATIVE_MINT
        || *dest_mint_info.key == spl_token_2022::native_mint::id();
    if !is_native_dest && !sell_config.is_whitelisted(dest_mint_info.key) {
        return Err(SplitterError::DestinationNotAllowed.into());
    }

    // --- Swap program must be approved ---
    if !sell_config.is_approved_program(swap_program_info.key) {
        return Err(SplitterError::SwapProgramNotApproved.into());
    }

    // --- Verify source token account belongs to splitter ---
    {
        let data = source_token_info.try_borrow_data()?;
        let acct = spl_token_2022::extension::StateWithExtensions::<spl_token_2022::state::Account>::unpack(&data)?;
        if acct.base.owner != *splitter_info.key {
            return Err(SplitterError::InvalidTokenAccountOwner.into());
        }
        if acct.base.mint != *source_mint_info.key {
            return Err(SplitterError::InvalidTokenAccountMint.into());
        }
    }

    // --- Record balances before ---
    let source_before = read_token_balance(source_token_info)?;
    let dest_before = read_token_balance(dest_token_info)?;

    if source_before == 0 {
        return Err(SplitterError::NothingToDistribute.into());
    }

    // --- Build CPI to swap program ---
    // All remaining accounts form the instruction for the swap program.
    // The cranker is responsible for ordering them correctly.
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

    let swap_ix = Instruction {
        program_id: *swap_program_info.key,
        accounts: cpi_account_metas,
        data: swap_data.to_vec(),
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

    // Pass all account infos so the runtime can find every referenced key.
    let mut cpi_infos: Vec<AccountInfo> = Vec::with_capacity(8 + remaining_accounts.len());
    cpi_infos.push(splitter_info.clone());
    cpi_infos.push(source_token_info.clone());
    cpi_infos.push(dest_token_info.clone());
    cpi_infos.push(source_mint_info.clone());
    cpi_infos.push(dest_mint_info.clone());
    cpi_infos.push(token_program_info.clone());
    cpi_infos.push(swap_program_info.clone());
    cpi_infos.push(sell_config_info.clone());
    cpi_infos.extend(remaining_accounts);

    invoke_signed(&swap_ix, &cpi_infos, &[signer_seeds])?;

    // --- Verify balances after ---
    let source_after = read_token_balance(source_token_info)?;
    let dest_after = read_token_balance(dest_token_info)?;

    if source_after >= source_before {
        return Err(SplitterError::SwapSourceNotDecreased.into());
    }
    if dest_after <= dest_before {
        return Err(SplitterError::SwapDestNotIncreased.into());
    }

    let tokens_sold = source_before - source_after;
    let tokens_received = dest_after - dest_before;

    // --- If destination is wSOL, close the account to unwrap to native SOL ---
    if is_native_dest {
        invoke_signed(
            &spl_token_2022::instruction::close_account(
                token_program_info.key,
                dest_token_info.key,   // account to close
                splitter_info.key,     // lamports destination
                splitter_info.key,     // owner
                &[],
            )?,
            &[
                dest_token_info.clone(),
                splitter_info.clone(),
                token_program_info.clone(),
            ],
            &[signer_seeds],
        )?;
        msg!("Swapped {} tokens → {} wSOL (unwrapped to SOL)", tokens_sold, tokens_received);
    } else {
        msg!("Swapped {} tokens → {} output tokens", tokens_sold, tokens_received);
    }

    Ok(())
}
