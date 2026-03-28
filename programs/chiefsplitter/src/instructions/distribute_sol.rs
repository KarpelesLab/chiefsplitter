use borsh::BorshDeserialize;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};

use crate::{
    error::SplitterError,
    events::emit_distribution,
    state::Splitter,
};

/// Distribute SOL from the splitter PDA to recipients (permissionless crank)
///
/// Accounts:
/// 0. `[writable]` Splitter
/// 1..N `[writable]` Recipient accounts (must match configured recipients in order)
pub fn process_distribute_sol(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;

    // Deserialize splitter (read-only borrow, then drop)
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

    // Calculate distributable SOL (everything above rent-exempt minimum)
    let rent = Rent::get()?;
    let rent_exempt = rent.minimum_balance(Splitter::LEN);
    let current_lamports = splitter_info.lamports();

    if current_lamports <= rent_exempt {
        return Err(SplitterError::NothingToDistribute.into());
    }

    let distributable = current_lamports - rent_exempt;

    // Collect and validate recipient accounts
    let num = splitter.num_recipients as usize;
    let mut recipient_infos: Vec<&AccountInfo> = Vec::with_capacity(num);
    for i in 0..num {
        let recipient_info = next_account_info(account_info_iter)
            .map_err(|_| SplitterError::RecipientCountMismatch)?;
        if *recipient_info.key != splitter.recipients[i].address {
            msg!(
                "Recipient {} expected {} got {}",
                i,
                splitter.recipients[i].address,
                recipient_info.key
            );
            return Err(SplitterError::RecipientAddressMismatch.into());
        }
        recipient_infos.push(recipient_info);
    }

    // Distribute SOL proportionally; dust stays on PDA for next distribution
    let splitter_key = *splitter_info.key;
    let mut total_sent: u64 = 0;

    for i in 0..num {
        let share = splitter.recipients[i].share as u64;
        let amount = (distributable as u128 * share as u128 / 10000) as u64;

        if amount > 0 {
            **splitter_info.try_borrow_mut_lamports()? -= amount;
            **recipient_infos[i].try_borrow_mut_lamports()? += amount;

            emit_distribution(&splitter_key, recipient_infos[i].key, amount, false);
            total_sent += amount;
        }
    }

    msg!("Distributed {} lamports to {} recipients", total_sent, num);

    Ok(())
}
