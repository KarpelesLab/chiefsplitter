//! ChiefSplitter: Configurable Fee Splitter for SOL and SPL Tokens
//!
//! A Solana program that allows anyone to create and configure fee splitters.
//! Each splitter is a PDA that can receive SOL or tokens and distribute them
//! to configured recipients based on percentage shares.

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::AccountInfo, entrypoint, entrypoint::ProgramResult, msg,
    program_error::ProgramError, pubkey::Pubkey,
};

pub mod error;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

solana_program::declare_id!("ChiefYGYadRjMCgMNqbbFV8GUfiP2TqfRWzcWNynEoPh");

/// Program instructions
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub enum SplitterInstruction {
    /// Create a new fee splitter
    ///
    /// Accounts:
    /// 0. `[writable]` Splitter PDA (["splitter", creator, nonce.to_le_bytes()])
    /// 1. `[writable, signer]` Creator/payer
    /// 2. `[]` System program
    CreateSplitter {
        /// Nonce for PDA derivation (allows multiple splitters per creator)
        nonce: u64,
    },

    /// Set the distribution configuration
    ///
    /// Accounts:
    /// 0. `[writable]` Splitter
    /// 1. `[signer]` Admin
    SetSplitterDistribution {
        /// List of (recipient_address, share_bps) pairs. Shares must total 10000.
        recipients: Vec<(Pubkey, u16)>,
    },

    /// Transfer admin to a new address
    ///
    /// Accounts:
    /// 0. `[writable]` Splitter
    /// 1. `[signer]` Current admin
    SetSplitterAdmin {
        new_admin: Pubkey,
    },

    /// Revoke admin (set to Pubkey::default(), irreversible)
    ///
    /// Accounts:
    /// 0. `[writable]` Splitter
    /// 1. `[signer]` Current admin
    RevokeSplitterAdmin,

    /// Distribute SOL from the splitter PDA to recipients
    /// Permissionless - anyone can crank this
    ///
    /// Accounts:
    /// 0. `[writable]` Splitter
    /// 1..N `[writable]` Recipient accounts (must match configured recipients in order)
    DistributeSOL,

    /// Distribute tokens from the splitter's token account to recipients
    /// Permissionless - anyone can crank this
    ///
    /// Accounts:
    /// 0. `[]` Splitter (read-only, for config + PDA signing)
    /// 1. `[]` Token mint
    /// 2. `[writable]` Splitter's token account
    /// 3. `[]` Token program (SPL Token or Token 2022)
    /// 4..N `[writable]` Recipient token accounts (must match configured recipients in order)
    DistributeToken,

    /// Lock a recipient to a guaranteed minimum share
    /// The recipient must sign and must already receive >= the specified share
    ///
    /// Accounts:
    /// 0. `[writable]` Splitter
    /// 1. `[signer]` Recipient
    LockRecipient {
        /// Minimum guaranteed share in basis points
        min_share: u16,
    },

    /// Create or update sell configuration (token whitelist for auto-sell)
    /// Whitelisted tokens are kept and distributed directly.
    /// Non-whitelisted tokens can be sold by a permissionless crank.
    ///
    /// Accounts:
    /// 0. `[]` Splitter
    /// 1. `[writable]` SellConfig PDA (["sell_config", splitter])
    /// 2. `[signer]` Admin
    /// 3. `[writable, signer]` Payer (for account creation if needed)
    /// 4. `[]` System program
    SetSellConfig {
        /// Token mints to whitelist (keep and distribute directly)
        whitelisted_mints: Vec<Pubkey>,
        /// Swap programs approved for CPI (Jupiter, Raydium, etc.)
        approved_swap_programs: Vec<Pubkey>,
    },

    /// Close sell configuration (disables selling, returns rent to admin)
    ///
    /// Accounts:
    /// 0. `[]` Splitter
    /// 1. `[writable]` SellConfig PDA
    /// 2. `[writable, signer]` Admin (receives rent refund)
    CloseSellConfig,

    /// Swap non-whitelisted tokens via an admin-approved DEX program
    /// (permissionless crank). Output must be native SOL (via wSOL, auto-unwrapped)
    /// or a whitelisted token.
    ///
    /// Accounts:
    /// 0. `[writable]` Splitter PDA (PDA signing + receives SOL on wSOL unwrap)
    /// 1. `[]` SellConfig PDA
    /// 2. `[]` Source token mint (verified not whitelisted)
    /// 3. `[writable]` Splitter's source token account
    /// 4. `[writable]` Splitter's destination token account (wSOL or whitelisted)
    /// 5. `[]` Destination token mint
    /// 6. `[]` Swap program (must be in approved list)
    /// 7. `[]` Token program (for wSOL close_account)
    /// 8..N Remaining accounts passed through to swap program CPI
    SwapToken {
        /// Raw instruction data forwarded to the swap program
        swap_data: Vec<u8>,
    },

    /// CPI proxy restricted to Bonfida SNS Name Service program.
    /// Allows the admin to set a primary .sol domain, update records, etc.
    /// Cannot touch tokens or SOL — only the SNS program is allowed.
    ///
    /// Accounts:
    /// 0. `[]` Splitter (config + PDA signing)
    /// 1. `[signer]` Admin
    /// 2. `[]` SNS Name Service program (must be namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX)
    /// 3..N Remaining accounts forwarded to SNS program
    SnsProxy {
        /// Raw instruction data forwarded to the SNS program
        sns_data: Vec<u8>,
    },
}

#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process_instruction);

#[cfg(not(feature = "no-entrypoint"))]
use solana_security_txt::security_txt;

#[cfg(not(feature = "no-entrypoint"))]
security_txt! {
    name: "ChiefSplitter",
    project_url: "https://github.com/KarpelesLab/chiefsplitter",
    contacts: "link:https://github.com/KarpelesLab/chiefsplitter/security/advisories",
    policy: "https://github.com/KarpelesLab/chiefsplitter/security/policy",
    source_code: "https://github.com/KarpelesLab/chiefsplitter"
}

/// Program entrypoint
pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    if program_id != &crate::id() {
        return Err(ProgramError::IncorrectProgramId);
    }

    let instruction = SplitterInstruction::try_from_slice(instruction_data)
        .map_err(|_| ProgramError::InvalidInstructionData)?;

    match instruction {
        SplitterInstruction::CreateSplitter { nonce } => {
            msg!("Instruction: CreateSplitter (nonce={})", nonce);
            process_create_splitter(program_id, accounts, nonce)
        }
        SplitterInstruction::SetSplitterDistribution { recipients } => {
            msg!("Instruction: SetSplitterDistribution ({} recipients)", recipients.len());
            process_set_distribution(program_id, accounts, &recipients)
        }
        SplitterInstruction::SetSplitterAdmin { new_admin } => {
            msg!("Instruction: SetSplitterAdmin");
            process_set_admin(program_id, accounts, new_admin)
        }
        SplitterInstruction::RevokeSplitterAdmin => {
            msg!("Instruction: RevokeSplitterAdmin");
            process_revoke_admin(program_id, accounts)
        }
        SplitterInstruction::DistributeSOL => {
            msg!("Instruction: DistributeSOL");
            process_distribute_sol(program_id, accounts)
        }
        SplitterInstruction::DistributeToken => {
            msg!("Instruction: DistributeToken");
            process_distribute_token(program_id, accounts)
        }
        SplitterInstruction::LockRecipient { min_share } => {
            msg!("Instruction: LockRecipient (min_share={})", min_share);
            process_lock_recipient(program_id, accounts, min_share)
        }
        SplitterInstruction::SetSellConfig { whitelisted_mints, approved_swap_programs } => {
            msg!("Instruction: SetSellConfig ({} mints, {} programs)", whitelisted_mints.len(), approved_swap_programs.len());
            process_set_sell_config(program_id, accounts, &whitelisted_mints, &approved_swap_programs)
        }
        SplitterInstruction::CloseSellConfig => {
            msg!("Instruction: CloseSellConfig");
            process_close_sell_config(program_id, accounts)
        }
        SplitterInstruction::SwapToken { swap_data } => {
            msg!("Instruction: SwapToken ({} bytes)", swap_data.len());
            process_swap_token(program_id, accounts, &swap_data)
        }
        SplitterInstruction::SnsProxy { sns_data } => {
            msg!("Instruction: SnsProxy ({} bytes)", sns_data.len());
            process_sns_proxy(program_id, accounts, &sns_data)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_serialization() {
        let instruction = SplitterInstruction::CreateSplitter { nonce: 42 };
        let serialized = borsh::to_vec(&instruction).unwrap();
        let deserialized: SplitterInstruction =
            BorshDeserialize::try_from_slice(&serialized).unwrap();
        match deserialized {
            SplitterInstruction::CreateSplitter { nonce } => assert_eq!(nonce, 42),
            _ => panic!("Wrong instruction type"),
        }
    }

    #[test]
    fn test_set_distribution_serialization() {
        let recipients = vec![
            (Pubkey::default(), 5000u16),
            (Pubkey::default(), 5000u16),
        ];
        let instruction = SplitterInstruction::SetSplitterDistribution { recipients };
        let serialized = borsh::to_vec(&instruction).unwrap();
        let deserialized: SplitterInstruction =
            BorshDeserialize::try_from_slice(&serialized).unwrap();
        match deserialized {
            SplitterInstruction::SetSplitterDistribution { recipients } => {
                assert_eq!(recipients.len(), 2);
                assert_eq!(recipients[0].1, 5000);
            }
            _ => panic!("Wrong instruction type"),
        }
    }
}
