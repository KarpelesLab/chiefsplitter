use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::pubkey::Pubkey;

/// Maximum number of recipients per splitter
pub const MAX_RECIPIENTS: usize = 10;

/// Maximum name length in bytes
pub const MAX_NAME_LEN: usize = 64;

/// Total shares must equal this value (100% in basis points = 1/100 of percent)
pub const TOTAL_SHARES: u16 = 10000;

/// Seed prefix for splitter PDAs
pub const SPLITTER_SEED: &[u8] = b"splitter";

/// Seed prefix for sell config PDAs
pub const SELL_CONFIG_SEED: &[u8] = b"sell_config";

/// Maximum number of whitelisted mints per sell config
pub const MAX_WHITELIST: usize = 10;

/// Maximum number of approved swap programs per sell config
pub const MAX_APPROVED_PROGRAMS: usize = 5;

/// The original SPL Token program ID (TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA)
pub const SPL_TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    0x06, 0xdd, 0xf6, 0xe1, 0xd7, 0x65, 0xa1, 0x93,
    0xd9, 0xcb, 0xe1, 0x46, 0xce, 0xeb, 0x79, 0xac,
    0x1c, 0xb4, 0x85, 0xed, 0x5f, 0x5b, 0x37, 0x91,
    0x3a, 0x8c, 0xf5, 0x85, 0x7e, 0xff, 0x00, 0xa9,
]);

/// Check if a program ID is a valid token program (SPL Token or Token 2022)
pub fn is_valid_token_program(key: &Pubkey) -> bool {
    *key == spl_token_2022::id() || *key == SPL_TOKEN_PROGRAM_ID
}

/// Bonfida SNS Name Service program ID (namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX)
pub const SNS_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    0x0b, 0xad, 0x51, 0xf4, 0x13, 0xc1, 0xf3, 0xa9,
    0x94, 0x60, 0xd9, 0x00, 0xd8, 0xbf, 0x2e, 0xd6,
    0x92, 0x7e, 0xca, 0x34, 0xd7, 0xb7, 0x84, 0x2b,
    0xf8, 0x10, 0xa9, 0x73, 0x08, 0x2d, 0x1e, 0xdc,
]);

/// Account discriminator for Splitter accounts
pub const SPLITTER_DISCRIMINATOR: [u8; 8] = [0xf1, 0x3a, 0x7c, 0x5e, 0x2b, 0x8d, 0x4f, 0x91];

/// Account discriminator for SellConfig accounts
pub const SELL_CONFIG_DISCRIMINATOR: [u8; 8] = [0xb2, 0x4d, 0x6a, 0x1f, 0x8c, 0x3e, 0x57, 0xa0];

/// A single recipient entry in the splitter
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, Copy, Default)]
pub struct Recipient {
    /// Recipient wallet address
    pub address: Pubkey,
    /// Share in basis points (1/100 of percent, 0-10000)
    pub share: u16,
    /// Minimum guaranteed share (0 = not locked). Once locked, cannot be reduced.
    pub locked_share: u16,
}

/// Splitter account state
/// PDA: ["splitter", creator, nonce.to_le_bytes()]
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct Splitter {
    /// Discriminator for account type identification
    pub discriminator: [u8; 8],
    /// Original creator (part of PDA derivation)
    pub creator: Pubkey,
    /// Current admin (can configure distribution, transfer admin, or revoke)
    pub admin: Pubkey,
    /// Nonce for PDA derivation (allows multiple splitters per creator)
    pub nonce: u64,
    /// PDA bump seed
    pub bump: u8,
    /// Number of active recipients
    pub num_recipients: u8,
    /// Fixed array of recipients (unused slots have address = Pubkey::default())
    pub recipients: [Recipient; MAX_RECIPIENTS],
    /// Actual byte length of name
    pub name_len: u8,
    /// UTF-8 name, zero-padded
    pub name: [u8; MAX_NAME_LEN],
}

impl Splitter {
    /// Size of the account in bytes
    pub const LEN: usize = 8 +   // discriminator
        32 +                      // creator
        32 +                      // admin
        8 +                       // nonce
        1 +                       // bump
        1 +                       // num_recipients
        MAX_RECIPIENTS * (32 + 2 + 2) + // recipients (36 bytes each)
        1 +                       // name_len
        MAX_NAME_LEN;             // name

    /// Create a new splitter with a name
    pub fn new(creator: Pubkey, nonce: u64, bump: u8, name: &[u8]) -> Self {
        let mut s = Self {
            discriminator: SPLITTER_DISCRIMINATOR,
            creator,
            admin: creator,
            nonce,
            bump,
            num_recipients: 1,
            recipients: [Recipient::default(); MAX_RECIPIENTS],
            name_len: 0,
            name: [0u8; MAX_NAME_LEN],
        };
        // Default: 100% to creator
        s.recipients[0] = Recipient {
            address: creator,
            share: TOTAL_SHARES,
            locked_share: 0,
        };
        s.set_name(name);
        s
    }

    /// Set the name (truncates to MAX_NAME_LEN)
    pub fn set_name(&mut self, name: &[u8]) {
        let len = name.len().min(MAX_NAME_LEN);
        self.name[..len].copy_from_slice(&name[..len]);
        self.name[len..].fill(0);
        self.name_len = len as u8;
    }

    /// Check if splitter is initialized
    pub fn is_initialized(&self) -> bool {
        self.discriminator == SPLITTER_DISCRIMINATOR
    }

    /// Check if admin has been revoked
    pub fn is_admin_revoked(&self) -> bool {
        self.admin == Pubkey::default()
    }

    /// Derive splitter PDA
    pub fn derive_pda(creator: &Pubkey, nonce: u64, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[SPLITTER_SEED, creator.as_ref(), &nonce.to_le_bytes()],
            program_id,
        )
    }

}

/// Sell configuration account - tracks which tokens to keep vs sell
/// PDA: ["sell_config", splitter]
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct SellConfig {
    /// Discriminator for account type identification
    pub discriminator: [u8; 8],
    /// Back-reference to the splitter this config belongs to
    pub splitter: Pubkey,
    /// Number of whitelisted mints
    pub num_whitelisted: u8,
    /// Whitelisted token mints (tokens to keep and distribute directly)
    /// Non-whitelisted tokens can be sold by a permissionless crank
    pub whitelisted_mints: [Pubkey; MAX_WHITELIST],
    /// Number of approved swap programs
    pub num_approved_programs: u8,
    /// Approved swap programs (Jupiter, Raydium, etc.)
    pub approved_programs: [Pubkey; MAX_APPROVED_PROGRAMS],
    /// PDA bump seed
    pub bump: u8,
}

impl SellConfig {
    /// Size of the account in bytes
    pub const LEN: usize = 8 +   // discriminator
        32 +                      // splitter
        1 +                       // num_whitelisted
        MAX_WHITELIST * 32 +      // whitelisted_mints
        1 +                       // num_approved_programs
        MAX_APPROVED_PROGRAMS * 32 + // approved_programs
        1;                        // bump

    /// Create a new sell config
    pub fn new(splitter: Pubkey, bump: u8) -> Self {
        Self {
            discriminator: SELL_CONFIG_DISCRIMINATOR,
            splitter,
            num_whitelisted: 0,
            whitelisted_mints: [Pubkey::default(); MAX_WHITELIST],
            num_approved_programs: 0,
            approved_programs: [Pubkey::default(); MAX_APPROVED_PROGRAMS],
            bump,
        }
    }

    /// Check if sell config is initialized
    pub fn is_initialized(&self) -> bool {
        self.discriminator == SELL_CONFIG_DISCRIMINATOR
    }

    /// Check if a mint is whitelisted (should be kept, not sold)
    pub fn is_whitelisted(&self, mint: &Pubkey) -> bool {
        for i in 0..self.num_whitelisted as usize {
            if self.whitelisted_mints[i] == *mint {
                return true;
            }
        }
        false
    }

    /// Check if a program is approved for swaps
    pub fn is_approved_program(&self, program_id: &Pubkey) -> bool {
        for i in 0..self.num_approved_programs as usize {
            if self.approved_programs[i] == *program_id {
                return true;
            }
        }
        false
    }

    /// Derive sell config PDA
    pub fn derive_pda(splitter: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[SELL_CONFIG_SEED, splitter.as_ref()],
            program_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_splitter_size() {
        let splitter = Splitter::new(Pubkey::default(), 0, 255, b"test");
        let serialized = borsh::to_vec(&splitter).unwrap();
        assert_eq!(serialized.len(), Splitter::LEN);
    }

    #[test]
    fn test_spl_token_program_id() {
        let expected: Pubkey = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
            .parse()
            .unwrap();
        assert_eq!(SPL_TOKEN_PROGRAM_ID, expected);
    }

    #[test]
    fn test_is_valid_token_program() {
        assert!(is_valid_token_program(&spl_token_2022::id()));
        assert!(is_valid_token_program(&SPL_TOKEN_PROGRAM_ID));
        assert!(!is_valid_token_program(&Pubkey::default()));
    }

    #[test]
    fn test_recipient_default() {
        let r = Recipient::default();
        assert_eq!(r.address, Pubkey::default());
        assert_eq!(r.share, 0);
        assert_eq!(r.locked_share, 0);
    }

    #[test]
    fn test_derive_pda_different_nonces() {
        let creator = Pubkey::new_unique();
        let program_id = Pubkey::new_unique();
        let (pda0, _) = Splitter::derive_pda(&creator, 0, &program_id);
        let (pda1, _) = Splitter::derive_pda(&creator, 1, &program_id);
        assert_ne!(pda0, pda1);
    }

    #[test]
    fn test_sell_config_size() {
        let config = SellConfig::new(Pubkey::default(), 255);
        let serialized = borsh::to_vec(&config).unwrap();
        assert_eq!(serialized.len(), SellConfig::LEN);
    }

    #[test]
    fn test_sell_config_whitelist() {
        let mut config = SellConfig::new(Pubkey::default(), 255);
        let mint1 = Pubkey::new_unique();
        let mint2 = Pubkey::new_unique();
        let mint3 = Pubkey::new_unique();

        config.whitelisted_mints[0] = mint1;
        config.whitelisted_mints[1] = mint2;
        config.num_whitelisted = 2;

        assert!(config.is_whitelisted(&mint1));
        assert!(config.is_whitelisted(&mint2));
        assert!(!config.is_whitelisted(&mint3));
    }

    #[test]
    fn test_sns_program_id() {
        let expected: Pubkey = "namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX"
            .parse()
            .unwrap();
        assert_eq!(SNS_PROGRAM_ID, expected);
    }

    #[test]
    fn test_sell_config_approved_programs() {
        let mut config = SellConfig::new(Pubkey::default(), 255);
        let prog1 = Pubkey::new_unique();
        let prog2 = Pubkey::new_unique();
        let prog3 = Pubkey::new_unique();

        config.approved_programs[0] = prog1;
        config.num_approved_programs = 1;

        assert!(config.is_approved_program(&prog1));
        assert!(!config.is_approved_program(&prog2));
        assert!(!config.is_approved_program(&prog3));
    }
}
