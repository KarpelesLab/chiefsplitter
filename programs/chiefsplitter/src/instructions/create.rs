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

#[cfg(test)]
mod tests {
    use borsh::BorshDeserialize;
    use solana_program::{
        instruction::{AccountMeta, Instruction, InstructionError},
        pubkey::Pubkey,
        rent::Rent,
        system_program,
    };
    use solana_program_test::{processor, BanksClientError, ProgramTest, ProgramTestContext};
    use solana_sdk::{
        account::Account,
        signature::{Keypair, Signer},
        transaction::{Transaction, TransactionError},
    };

    use crate::{
        error::SplitterError,
        state::{Splitter, TOTAL_SHARES},
        SplitterInstruction,
    };

    fn program_test() -> ProgramTest {
        ProgramTest::new(
            "chiefsplitter",
            crate::id(),
            processor!(crate::process_instruction),
        )
    }

    fn system_account(lamports: u64) -> Account {
        Account {
            lamports,
            owner: system_program::id(),
            ..Account::default()
        }
    }

    fn create_ix(creator: &Pubkey, nonce: u64) -> (Pubkey, Instruction) {
        let (pda, _) = Splitter::derive_pda(creator, nonce, &crate::id());
        let data = borsh::to_vec(&SplitterInstruction::CreateSplitter {
            nonce,
            name: "test".to_string(),
        })
        .unwrap();
        let ix = Instruction {
            program_id: crate::id(),
            accounts: vec![
                AccountMeta::new(pda, false),
                AccountMeta::new(*creator, true),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            data,
        };
        (pda, ix)
    }

    async fn send(
        ctx: &mut ProgramTestContext,
        creator: &Keypair,
        ix: Instruction,
    ) -> Result<(), BanksClientError> {
        let blockhash = ctx.banks_client.get_latest_blockhash().await.unwrap();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&creator.pubkey()),
            &[creator],
            blockhash,
        );
        ctx.banks_client.process_transaction(tx).await
    }

    async fn assert_created(ctx: &mut ProgramTestContext, pda: Pubkey, creator: &Pubkey) -> u64 {
        let account = ctx.banks_client.get_account(pda).await.unwrap().unwrap();
        assert_eq!(account.owner, crate::id());
        assert_eq!(account.data.len(), Splitter::LEN);
        let splitter = Splitter::try_from_slice(&account.data).unwrap();
        assert!(splitter.is_initialized());
        assert_eq!(splitter.creator, *creator);
        assert_eq!(splitter.admin, *creator);
        assert_eq!(splitter.num_recipients, 1);
        assert_eq!(splitter.recipients[0].address, *creator);
        assert_eq!(splitter.recipients[0].share, TOTAL_SHARES);
        assert_eq!(&splitter.name[..splitter.name_len as usize], b"test");
        account.lamports
    }

    /// Run CreateSplitter against a PDA that already holds `prefunded` lamports
    /// and return (final PDA lamports, rent-exempt minimum).
    async fn create_with_prefund(prefunded: u64) -> (u64, u64) {
        let creator = Keypair::new();
        let (pda, ix) = create_ix(&creator.pubkey(), 7);

        let mut pt = program_test();
        pt.add_account(creator.pubkey(), system_account(10_000_000_000));
        if prefunded > 0 {
            pt.add_account(pda, system_account(prefunded));
        }
        let mut ctx = pt.start_with_context().await;

        send(&mut ctx, &creator, ix).await.unwrap();

        let lamports = assert_created(&mut ctx, pda, &creator.pubkey()).await;
        let rent = ctx.banks_client.get_rent().await.unwrap();
        (lamports, rent.minimum_balance(Splitter::LEN))
    }

    #[tokio::test]
    async fn create_fresh_pda() {
        let (lamports, min) = create_with_prefund(0).await;
        assert_eq!(lamports, min);
    }

    #[tokio::test]
    async fn create_prefunded_below_rent_tops_up() {
        // A single donated lamport used to make create_account fail forever
        let (lamports, min) = create_with_prefund(1).await;
        assert_eq!(lamports, min);
    }

    #[tokio::test]
    async fn create_prefunded_exactly_rent() {
        let min = Rent::default().minimum_balance(Splitter::LEN);
        let (lamports, actual_min) = create_with_prefund(min).await;
        assert_eq!(min, actual_min);
        assert_eq!(lamports, min);
    }

    #[tokio::test]
    async fn create_prefunded_above_rent_keeps_excess() {
        let (lamports, min) = create_with_prefund(5_000_000_000).await;
        assert!(min < 5_000_000_000);
        assert_eq!(lamports, 5_000_000_000);
    }

    #[tokio::test]
    async fn create_twice_is_rejected() {
        let creator = Keypair::new();
        let (pda, ix) = create_ix(&creator.pubkey(), 0);

        let mut pt = program_test();
        pt.add_account(creator.pubkey(), system_account(10_000_000_000));
        pt.add_account(pda, system_account(1));
        let mut ctx = pt.start_with_context().await;

        send(&mut ctx, &creator, ix.clone()).await.unwrap();
        assert_created(&mut ctx, pda, &creator.pubkey()).await;

        // Move to a new blockhash so the second transaction is not a duplicate
        ctx.warp_to_slot(10).unwrap();
        let err = send(&mut ctx, &creator, ix).await.unwrap_err().unwrap();
        assert_eq!(
            err,
            TransactionError::InstructionError(
                0,
                InstructionError::Custom(SplitterError::AlreadyInitialized as u32)
            )
        );
    }
}
