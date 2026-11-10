use borsh::BorshDeserialize;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
};

use crate::{
    error::SplitterError,
    state::{is_valid_token_program, Splitter, SNS_NAME_OFFERS_ID, SNS_PROGRAM_ID, SPLITTER_SEED},
};

/// Associated Token Account program (ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL).
/// Never forwarded to an SNS CPI.
const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

// Instruction allow-list.
//
// The first byte of the instruction data of both target programs is a u8
// instruction tag. Only the tags below may be forwarded. Tag values were
// verified against the published Bonfida SDKs:
//
// * SNS Name Service (namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX):
//   `@bonfida/spl-name-service` 4.0.1, `dist/esm/instructions/` —
//   createInstruction = 0, updateInstruction = 1, transferInstruction = 2,
//   deleteInstruction = 3, reallocInstruction = 4.
//
// * Name Offers (85iDfUvr3HJyLM2zcq5BXSiDvUWfw6cSE1FfNBo8Ap29):
//   `@bonfida/name-offers` 0.3.0-alpha.4, `dist/index.mjs` —
//   `registerFavouriteInstruction` has `tag = 6` and
//   `unregisterFavouriteInstruction` has `tag = 13` (both serialize as a single
//   `tag: u8`). Cross-checked with `@bonfida/spl-name-service` 4.0.1
//   `SetPrimaryInstruction` (`tag = 6`), which `setPrimaryDomain` sends to
//   the Name Offers program id.
//
// Everything else — in particular the marketplace instructions of Name Offers
// (BuyFixedPrice = 5, MakeOffer, AcceptOffer, ...), which move SPL tokens on the
// signer's authority, and name service Transfer / Delete — is rejected.

/// SNS Name Service `Update` (write data into a name record).
const NAME_SERVICE_IX_UPDATE: u8 = 1;
/// Name Offers `RegisterFavourite` (set primary domain).
const NAME_OFFERS_IX_REGISTER_FAVOURITE: u8 = 6;
/// Name Offers `UnregisterFavourite` (clear primary domain).
const NAME_OFFERS_IX_UNREGISTER_FAVOURITE: u8 = 13;

/// Returns true if `sns_data` is an allowed instruction for `sns_program`.
/// Empty data is never allowed.
fn is_allowed_sns_instruction(sns_program: &Pubkey, sns_data: &[u8]) -> bool {
    let Some(&tag) = sns_data.first() else {
        return false;
    };
    if *sns_program == SNS_PROGRAM_ID {
        tag == NAME_SERVICE_IX_UPDATE
    } else if *sns_program == SNS_NAME_OFFERS_ID {
        tag == NAME_OFFERS_IX_REGISTER_FAVOURITE || tag == NAME_OFFERS_IX_UNREGISTER_FAVOURITE
    } else {
        false
    }
}

/// Returns true if an account must never be forwarded to the SNS CPI:
/// a token program, the associated-token program, or any account owned by a
/// token program (token accounts, mints, multisigs).
fn is_token_related(key: &Pubkey, owner: &Pubkey) -> bool {
    is_valid_token_program(key)
        || *key == ASSOCIATED_TOKEN_PROGRAM_ID
        || is_valid_token_program(owner)
}

/// CPI proxy restricted to the Bonfida SNS programs: Name Service
/// (namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX, `Update` only) and Name Offers
/// (85iDfUvr3HJyLM2zcq5BXSiDvUWfw6cSE1FfNBo8Ap29, `RegisterFavourite` /
/// `UnregisterFavourite` only).
///
/// Allows the admin to set / clear the splitter's primary .sol domain and
/// update name records. The proxy cannot touch tokens or SOL: no token program,
/// associated-token program, or token-program-owned account may be forwarded,
/// and the splitter's lamport balance must not decrease.
///
/// Accounts:
/// 0. `[]` Splitter (config + PDA signing)
/// 1. `[signer]` Admin
/// 2. `[]` SNS program (Name Service or Name Offers)
/// 3..N Remaining accounts forwarded to the SNS program
pub fn process_sns_proxy(
    program_id: &Pubkey,
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

    // The splitter must be an account of this program
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

    // MUST be a Bonfida SNS program — nothing else
    if *sns_program_info.key != SNS_PROGRAM_ID && *sns_program_info.key != SNS_NAME_OFFERS_ID {
        msg!("SNS proxy only allows CPI to SNS programs");
        return Err(SplitterError::InvalidPDA.into());
    }

    // Only allow-listed instructions (also rejects empty data)
    if !is_allowed_sns_instruction(sns_program_info.key, sns_data) {
        msg!("SNS proxy: instruction not allowed for this program");
        return Err(ProgramError::InvalidInstructionData);
    }

    // The callee can only touch accounts that are passed to it. Refuse to
    // forward anything token related, so the PDA signature can never be used
    // to move the splitter's tokens — whatever the callee does.
    for a in remaining_accounts.iter() {
        if is_token_related(a.key, a.owner) {
            msg!("SNS proxy: token programs and token accounts cannot be forwarded");
            return Err(SplitterError::InvalidTokenProgram.into());
        }
    }

    // Build CPI instruction from remaining accounts.
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

    let sns_ix = Instruction {
        program_id: *sns_program_info.key,
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

    // The provided account must be the PDA these seeds sign for
    let expected_splitter = Pubkey::create_program_address(signer_seeds, program_id)
        .map_err(|_| SplitterError::InvalidPDA)?;
    if expected_splitter != *splitter_info.key {
        return Err(SplitterError::InvalidPDA.into());
    }

    // Include splitter PDA + all remaining accounts for invoke_signed
    let mut cpi_infos: Vec<AccountInfo> = Vec::with_capacity(2 + remaining_accounts.len());
    cpi_infos.push(splitter_info.clone());
    cpi_infos.push(sns_program_info.clone());
    cpi_infos.extend(remaining_accounts);

    let lamports_before = splitter_info.lamports();

    invoke_signed(&sns_ix, &cpi_infos, &[signer_seeds])?;

    // The proxy must never cost the splitter SOL (increases, e.g. rent
    // refunds, are fine).
    if splitter_info.lamports() < lamports_before {
        msg!("SNS proxy: splitter lamports decreased");
        return Err(ProgramError::InvalidAccountData);
    }

    msg!("SNS proxy CPI executed");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SPL_TOKEN_PROGRAM_ID;

    #[test]
    fn test_empty_data_rejected() {
        assert!(!is_allowed_sns_instruction(&SNS_PROGRAM_ID, &[]));
        assert!(!is_allowed_sns_instruction(&SNS_NAME_OFFERS_ID, &[]));
    }

    #[test]
    fn test_name_service_allow_list() {
        for tag in 0..=255u8 {
            assert_eq!(
                is_allowed_sns_instruction(&SNS_PROGRAM_ID, &[tag, 0, 0]),
                tag == 1,
                "name service tag {}",
                tag
            );
        }
    }

    #[test]
    fn test_name_offers_allow_list() {
        for tag in 0..=255u8 {
            assert_eq!(
                is_allowed_sns_instruction(&SNS_NAME_OFFERS_ID, &[tag]),
                tag == 6 || tag == 13,
                "name offers tag {}",
                tag
            );
        }
    }

    #[test]
    fn test_other_program_rejected() {
        assert!(!is_allowed_sns_instruction(&Pubkey::new_unique(), &[1]));
        assert!(!is_allowed_sns_instruction(&Pubkey::default(), &[6]));
    }

    #[test]
    fn test_is_token_related() {
        let other = Pubkey::new_unique();
        let system = Pubkey::default();
        assert!(!is_token_related(&other, &system));
        assert!(!is_token_related(&other, &SNS_PROGRAM_ID));
        // Token programs themselves
        assert!(is_token_related(&SPL_TOKEN_PROGRAM_ID, &other));
        assert!(is_token_related(&spl_token_2022::id(), &other));
        assert!(is_token_related(&ASSOCIATED_TOKEN_PROGRAM_ID, &other));
        // Accounts owned by a token program
        assert!(is_token_related(&other, &SPL_TOKEN_PROGRAM_ID));
        assert!(is_token_related(&other, &spl_token_2022::id()));
    }
}
