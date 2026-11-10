use borsh::BorshDeserialize;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::invoke_signed,
    program_error::ProgramError,
    program_option::COption,
    pubkey::Pubkey,
    system_program,
};
use spl_token_2022::{extension::StateWithExtensions, state::Account as TokenAccount};

use crate::{
    error::SplitterError,
    state::{
        is_valid_token_program, SellConfig, Splitter, SELL_CONFIG_SEED, SPLITTER_SEED,
        SPL_TOKEN_PROGRAM_ID,
    },
};

/// Associated Token Account program (ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL)
const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

/// Number of fixed accounts preceding the accounts forwarded to the swap program.
const FIXED_ACCOUNTS: usize = 8;

/// Byte range of the `owner` field inside an SPL token account.
const TOKEN_ACCOUNT_OWNER_RANGE: core::ops::Range<usize> = 32..64;

/// Programs that must never be used as a "swap program".
///
/// The swap CPI is signed by the splitter PDA with caller-supplied data, so
/// letting it target the token programs (direct `Transfer` / `SetAuthority` /
/// `Approve`), the system program, the associated token account program or
/// this program itself would hand the PDA signature to the cranker.
pub(crate) fn is_forbidden_swap_program(key: &Pubkey, program_id: &Pubkey) -> bool {
    *key == SPL_TOKEN_PROGRAM_ID
        || *key == spl_token_2022::id()
        || *key == system_program::id()
        || *key == ASSOCIATED_TOKEN_PROGRAM_ID
        || *key == *program_id
}

/// The security-relevant fields of a token account, captured so they can be
/// compared across the swap CPI.
struct TokenSnapshot {
    /// Program owning the account (SPL Token or Token-2022)
    program: Pubkey,
    mint: Pubkey,
    owner: Pubkey,
    amount: u64,
    delegate: COption<Pubkey>,
    delegated_amount: u64,
    close_authority: COption<Pubkey>,
}

/// Read a token account, enforcing that it is owned by a valid token program.
fn read_token_account(account: &AccountInfo) -> Result<TokenSnapshot, ProgramError> {
    if !is_valid_token_program(account.owner) {
        return Err(SplitterError::InvalidTokenProgram.into());
    }
    let data = account.try_borrow_data()?;
    let acct = StateWithExtensions::<TokenAccount>::unpack(&data)?;
    Ok(TokenSnapshot {
        program: *account.owner,
        mint: acct.base.mint,
        owner: acct.base.owner,
        amount: acct.base.amount,
        delegate: acct.base.delegate,
        delegated_amount: acct.base.delegated_amount,
        close_authority: acct.base.close_authority,
    })
}

/// If `account` is a token account whose token owner is the splitter, return
/// a snapshot of it. Anything else (wrong program owner, not a token account,
/// owned by someone else) yields `None`.
fn snapshot_if_splitter_owned(
    account: &AccountInfo,
    splitter_key: &Pubkey,
) -> Result<Option<TokenSnapshot>, ProgramError> {
    if !is_valid_token_program(account.owner) {
        return Ok(None);
    }
    {
        // Cheap pre-filter on the raw owner field so pool vaults and other
        // third-party token accounts don't pay for a full unpack.
        let data = account.try_borrow_data()?;
        match data.get(TOKEN_ACCOUNT_OWNER_RANGE) {
            Some(owner) if owner == splitter_key.as_ref() => {}
            _ => return Ok(None),
        }
    }
    match read_token_account(account) {
        Ok(snapshot) if snapshot.owner == *splitter_key => Ok(Some(snapshot)),
        _ => Ok(None),
    }
}

/// Verify that the authorities over a splitter-owned token account were not
/// handed to anyone by the swap CPI: still under the same token program,
/// still owned by the splitter, no new/changed delegate, no larger delegated
/// allowance, same close authority.
///
/// `allow_delegate_cleared` permits a pre-existing delegate being revoked
/// (used for the declared source/destination accounts).
fn check_authorities_unchanged(
    before: &TokenSnapshot,
    after: &TokenSnapshot,
    splitter_key: &Pubkey,
    allow_delegate_cleared: bool,
) -> ProgramResult {
    if after.program != before.program || after.mint != before.mint {
        msg!("Swap changed the program owner or mint of a splitter token account");
        return Err(SplitterError::InvalidTokenAccountMint.into());
    }
    if after.owner != *splitter_key {
        msg!("Swap changed the owner of a splitter token account");
        return Err(SplitterError::InvalidTokenAccountOwner.into());
    }
    let delegate_cleared = allow_delegate_cleared && after.delegate.is_none();
    if !delegate_cleared
        && (after.delegate != before.delegate || after.delegated_amount > before.delegated_amount)
    {
        msg!("Swap changed the delegate of a splitter token account");
        return Err(SplitterError::InvalidTokenAccountOwner.into());
    }
    if after.close_authority != before.close_authority {
        msg!("Swap changed the close authority of a splitter token account");
        return Err(SplitterError::InvalidTokenAccountOwner.into());
    }
    Ok(())
}

/// Swap non-whitelisted tokens via an admin-approved DEX (permissionless crank).
///
/// The cranker builds the swap instruction off-chain (e.g. via Jupiter SDK)
/// and passes the raw data + all required accounts. Our program:
///   1. Validates the splitter, sell config, source mint, dest mint, and swap program
///   2. Records balances before, and snapshots every other splitter-owned
///      token account forwarded to the swap program
///   3. CPI to the swap program (invoke_signed — PDA is the authority)
///   4. Verifies source decreased and dest increased, and that no other
///      splitter-owned token account lost funds or changed authorities
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

    // Remaining accounts are forwarded to the swap program
    let remaining_accounts = &accounts[FIXED_ACCOUNTS..];

    // --- Validate splitter ---
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

    // Signer seeds for the splitter PDA; the account must be that PDA.
    let nonce_bytes = splitter.nonce.to_le_bytes();
    let bump_slice = [splitter.bump];
    let signer_seeds: &[&[u8]] = &[
        SPLITTER_SEED,
        splitter.creator.as_ref(),
        &nonce_bytes,
        &bump_slice,
    ];
    let expected_splitter = Pubkey::create_program_address(signer_seeds, program_id)
        .map_err(|_| SplitterError::InvalidPDA)?;
    if expected_splitter != *splitter_info.key {
        return Err(SplitterError::InvalidPDA.into());
    }

    // --- Validate sell config ---
    let (expected_config_pda, _) =
        Pubkey::find_program_address(&[SELL_CONFIG_SEED, splitter_info.key.as_ref()], program_id);
    if *sell_config_info.key != expected_config_pda {
        return Err(SplitterError::InvalidPDA.into());
    }
    if sell_config_info.owner != program_id {
        return Err(SplitterError::SellConfigNotFound.into());
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
        0x06, 0x9b, 0x88, 0x57, 0xfe, 0xab, 0x81, 0x84, 0xfb, 0x68, 0x7f, 0x63, 0x46, 0x18, 0xc0,
        0x35, 0xda, 0xc4, 0x39, 0xdc, 0x1a, 0xeb, 0x3b, 0x55, 0x98, 0xa0, 0xf0, 0x00, 0x00, 0x00,
        0x00, 0x01,
    ]);
    let is_native_dest = *dest_mint_info.key == SPL_NATIVE_MINT
        || *dest_mint_info.key == spl_token_2022::native_mint::id();
    if !is_native_dest && !sell_config.is_whitelisted(dest_mint_info.key) {
        return Err(SplitterError::DestinationNotAllowed.into());
    }

    // --- Swap program must be approved ---
    // The denylist is re-checked here (not only in SetSellConfig) to cover
    // sell configs stored on-chain before the denylist existed.
    if is_forbidden_swap_program(swap_program_info.key, program_id)
        || !sell_config.is_approved_program(swap_program_info.key)
    {
        return Err(SplitterError::SwapProgramNotApproved.into());
    }

    // --- Verify source token account belongs to splitter ---
    // `read_token_account` enforces that the account is owned by a token
    // program, so the data cannot be forged by an arbitrary program.
    let source_pre = read_token_account(source_token_info)?;
    if source_pre.owner != *splitter_info.key {
        return Err(SplitterError::InvalidTokenAccountOwner.into());
    }
    if source_pre.mint != *source_mint_info.key {
        return Err(SplitterError::InvalidTokenAccountMint.into());
    }
    if source_mint_info.owner != source_token_info.owner {
        return Err(SplitterError::InvalidTokenProgram.into());
    }

    // --- Verify destination token account belongs to splitter ---
    // The swap CPI runs with the splitter's signer privilege, so the proceeds
    // must land in an account the splitter owns (not one picked by the cranker).
    let dest_pre = read_token_account(dest_token_info)?;
    if dest_pre.owner != *splitter_info.key {
        return Err(SplitterError::InvalidTokenAccountOwner.into());
    }
    if dest_pre.mint != *dest_mint_info.key {
        return Err(SplitterError::InvalidTokenAccountMint.into());
    }
    if dest_mint_info.owner != dest_token_info.owner {
        return Err(SplitterError::InvalidTokenProgram.into());
    }
    // The wSOL unwrap below is a CPI into `token_program_info`; it must be the
    // program that actually owns the destination account.
    if is_native_dest && dest_token_info.owner != token_program_info.key {
        return Err(SplitterError::InvalidTokenProgram.into());
    }

    // The swap program must be an actual program.
    if !swap_program_info.executable {
        return Err(ProgramError::IncorrectProgramId);
    }

    if source_pre.amount == 0 {
        return Err(SplitterError::NothingToDistribute.into());
    }

    // --- Snapshot every other splitter-owned token account ---
    // The PDA signature covers the whole CPI, so a multi-leg route could move
    // funds out of (or re-assign authorities on) splitter token accounts other
    // than the declared source/destination — including whitelisted mints.
    // Only accounts forwarded to the swap program can be touched by it, so
    // snapshot those and compare after the CPI. Intermediate accounts of a
    // multi-hop route remain usable as long as they do not end up lower.
    let mut guarded: Vec<(usize, TokenSnapshot)> = Vec::new();
    for (index, account) in remaining_accounts.iter().enumerate() {
        if account.key == source_token_info.key || account.key == dest_token_info.key {
            continue;
        }
        if guarded
            .iter()
            .any(|(seen, _)| remaining_accounts[*seen].key == account.key)
        {
            continue;
        }
        if let Some(snapshot) = snapshot_if_splitter_owned(account, splitter_info.key)? {
            guarded.push((index, snapshot));
        }
    }

    let splitter_lamports_before = splitter_info.lamports();

    // --- Build CPI to swap program ---
    // All remaining accounts form the instruction for the swap program.
    // The cranker is responsible for ordering them correctly.
    // Mark the splitter PDA as signer — invoke_signed provides the signature.
    let cpi_account_metas: Vec<AccountMeta> = remaining_accounts
        .iter()
        .map(|a| {
            let is_pda = *a.key == *splitter_info.key;
            if a.is_writable {
                AccountMeta::new(*a.key, is_pda)
            } else {
                AccountMeta::new_readonly(*a.key, is_pda)
            }
        })
        .collect();

    let swap_ix = Instruction {
        program_id: *swap_program_info.key,
        accounts: cpi_account_metas,
        data: swap_data.to_vec(),
    };

    // Pass all account infos so the runtime can find every referenced key.
    // No account data borrows are held at this point.
    let mut cpi_infos: Vec<AccountInfo> =
        Vec::with_capacity(FIXED_ACCOUNTS + remaining_accounts.len());
    cpi_infos.push(splitter_info.clone());
    cpi_infos.push(source_token_info.clone());
    cpi_infos.push(dest_token_info.clone());
    cpi_infos.push(source_mint_info.clone());
    cpi_infos.push(dest_mint_info.clone());
    cpi_infos.push(token_program_info.clone());
    cpi_infos.push(swap_program_info.clone());
    cpi_infos.push(sell_config_info.clone());
    cpi_infos.extend(remaining_accounts.iter().cloned());

    invoke_signed(&swap_ix, &cpi_infos, &[signer_seeds])?;

    // --- Verify the splitter itself after ---
    if splitter_info.owner != program_id {
        return Err(SplitterError::InvalidAccountOwner.into());
    }
    if splitter_info.lamports() < splitter_lamports_before {
        msg!("Swap decreased the splitter's SOL balance");
        return Err(ProgramError::InsufficientFunds);
    }

    // --- Verify balances after ---
    // `read_token_account` re-checks token program ownership post-CPI.
    let source_post = read_token_account(source_token_info)?;
    let dest_post = read_token_account(dest_token_info)?;

    if source_post.amount >= source_pre.amount {
        return Err(SplitterError::SwapSourceNotDecreased.into());
    }
    if dest_post.amount <= dest_pre.amount {
        return Err(SplitterError::SwapDestNotIncreased.into());
    }

    // --- Verify the declared accounts were not handed to someone else ---
    check_authorities_unchanged(&source_pre, &source_post, splitter_info.key, true)?;
    check_authorities_unchanged(&dest_pre, &dest_post, splitter_info.key, true)?;

    // --- Verify every other splitter-owned token account ---
    for (index, before) in guarded.iter() {
        let account = &remaining_accounts[*index];
        // Fails if the account was closed or is no longer a token account.
        let after = read_token_account(account)?;
        check_authorities_unchanged(before, &after, splitter_info.key, false)?;
        if after.amount < before.amount {
            msg!("Swap decreased an undeclared splitter token account");
            return Err(ProgramError::InsufficientFunds);
        }
    }

    let tokens_sold = source_pre.amount - source_post.amount;
    let tokens_received = dest_post.amount - dest_pre.amount;

    // --- If destination is wSOL, close the account to unwrap to native SOL ---
    if is_native_dest {
        invoke_signed(
            &spl_token_2022::instruction::close_account(
                token_program_info.key,
                dest_token_info.key, // account to close
                splitter_info.key,   // lamports destination
                splitter_info.key,   // owner
                &[],
            )?,
            &[
                dest_token_info.clone(),
                splitter_info.clone(),
                token_program_info.clone(),
            ],
            &[signer_seeds],
        )?;
        msg!(
            "Swapped {} tokens → {} wSOL (unwrapped to SOL)",
            tokens_sold,
            tokens_received
        );
    } else {
        msg!(
            "Swapped {} tokens → {} output tokens",
            tokens_sold,
            tokens_received
        );
    }

    Ok(())
}
