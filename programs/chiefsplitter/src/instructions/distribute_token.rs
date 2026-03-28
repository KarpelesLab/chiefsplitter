use borsh::BorshDeserialize;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program::invoke_signed,
    program_pack::Pack,
    pubkey::Pubkey,
};
use spl_token_2022::state::Mint;

use crate::{
    error::SplitterError,
    events::emit_distribution,
    state::{is_valid_token_program, Splitter, SPLITTER_SEED},
};

/// Distribute tokens from the splitter's token account to recipients (permissionless crank)
///
/// Accounts:
/// 0. `[]` Splitter (read-only, config + PDA signing authority)
/// 1. `[]` Token mint
/// 2. `[writable]` Splitter's token account
/// 3. `[]` Token program (SPL Token or Token 2022)
/// 4..N `[writable]` Recipient token accounts (must match configured recipients in order)
pub fn process_distribute_token(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let mint_info = next_account_info(account_info_iter)?;
    let splitter_token_info = next_account_info(account_info_iter)?;
    let token_program_info = next_account_info(account_info_iter)?;

    // Validate token program
    if !is_valid_token_program(token_program_info.key) {
        return Err(SplitterError::InvalidTokenProgram.into());
    }

    // Deserialize splitter
    let splitter = {
        let data = splitter_info.try_borrow_data()?;
        Splitter::try_from_slice(&data).map_err(|_| SplitterError::NotInitialized)?
    };

    if !splitter.is_initialized() {
        return Err(SplitterError::NotInitialized.into());
    }

    if splitter.num_recipients == 0 {
        return Err(SplitterError::NoRecipients.into());
    }

    // Validate the splitter's token account is owned by the token program
    if *splitter_token_info.owner != *token_program_info.key {
        return Err(SplitterError::InvalidTokenAccountOwner.into());
    }

    // Unpack the token account to get balance and verify owner/mint
    let token_account = {
        let data = splitter_token_info.try_borrow_data()?;
        spl_token_2022::extension::StateWithExtensions::<spl_token_2022::state::Account>::unpack(&data)?.base
    };

    // Verify the token account is owned by the splitter PDA
    if token_account.owner != *splitter_info.key {
        return Err(SplitterError::InvalidTokenAccountOwner.into());
    }

    // Verify the token account mint matches
    if token_account.mint != *mint_info.key {
        return Err(SplitterError::InvalidTokenAccountMint.into());
    }

    let total_balance = token_account.amount;
    if total_balance == 0 {
        return Err(SplitterError::NothingToDistribute.into());
    }

    // Get mint decimals for transfer_checked
    let decimals = {
        let mint_data = mint_info.try_borrow_data()?;
        Mint::unpack(&mint_data)?.decimals
    };

    // Collect recipient token accounts
    let num = splitter.num_recipients as usize;
    let mut recipient_token_infos: Vec<&AccountInfo> = Vec::with_capacity(num);
    for _ in 0..num {
        let recipient_token_info = next_account_info(account_info_iter)
            .map_err(|_| SplitterError::RecipientCountMismatch)?;
        recipient_token_infos.push(recipient_token_info);
    }

    // Build signer seeds for the splitter PDA
    let nonce_bytes = splitter.nonce.to_le_bytes();
    let bump_slice = [splitter.bump];
    let signer_seeds: &[&[u8]] = &[
        SPLITTER_SEED,
        splitter.creator.as_ref(),
        &nonce_bytes,
        &bump_slice,
    ];

    // Distribute tokens proportionally; dust stays on PDA for next distribution
    let splitter_key = *splitter_info.key;
    let mut total_sent: u64 = 0;

    for i in 0..num {
        let share = splitter.recipients[i].share as u64;
        let amount = (total_balance as u128 * share as u128 / 10000) as u64;

        if amount > 0 {
            invoke_signed(
                &spl_token_2022::instruction::transfer_checked(
                    token_program_info.key,
                    splitter_token_info.key,
                    mint_info.key,
                    recipient_token_infos[i].key,
                    splitter_info.key,
                    &[],
                    amount,
                    decimals,
                )?,
                &[
                    splitter_token_info.clone(),
                    mint_info.clone(),
                    recipient_token_infos[i].clone(),
                    splitter_info.clone(),
                ],
                &[signer_seeds],
            )?;

            emit_distribution(
                &splitter_key,
                &splitter.recipients[i].address,
                amount,
                true,
            );
            total_sent += amount;
        }
    }

    msg!(
        "Distributed {} tokens to {} recipients",
        total_sent,
        num
    );

    Ok(())
}
