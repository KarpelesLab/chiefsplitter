use solana_program::program_error::ProgramError;
use thiserror::Error;

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitterError {
    #[error("Account already initialized")]
    AlreadyInitialized,

    #[error("Account not initialized")]
    NotInitialized,

    #[error("Invalid PDA")]
    InvalidPDA,

    #[error("Missing required signer")]
    MissingRequiredSigner,

    #[error("Invalid authority - signer is not the admin")]
    InvalidAuthority,

    #[error("Admin has been revoked")]
    AdminRevoked,

    #[error("Distribution shares must total 10000 (100%)")]
    InvalidShareTotal,

    #[error("Too many recipients (max 10)")]
    TooManyRecipients,

    #[error("Cannot reduce locked recipient below locked share")]
    LockedShareViolation,

    #[error("No recipients configured")]
    NoRecipients,

    #[error("Nothing to distribute")]
    NothingToDistribute,

    #[error("Recipient count mismatch with provided accounts")]
    RecipientCountMismatch,

    #[error("Recipient address mismatch")]
    RecipientAddressMismatch,

    #[error("Invalid token program - must be SPL Token or Token 2022")]
    InvalidTokenProgram,

    #[error("Invalid token account owner")]
    InvalidTokenAccountOwner,

    #[error("Invalid token account mint")]
    InvalidTokenAccountMint,

    #[error("Signer is not a configured recipient")]
    NotARecipient,

    #[error("Recipient already has a higher or equal locked share")]
    LockShareNotIncreased,

    #[error("Lock share exceeds current share")]
    LockShareExceedsCurrent,

    #[error("Zero share not allowed for a recipient")]
    ZeroShare,

    #[error("Invalid account owner")]
    InvalidAccountOwner,

    #[error("Too many whitelisted mints (max 10)")]
    TooManyWhitelistedMints,

    #[error("Sell config not found - selling not enabled")]
    SellConfigNotFound,

    #[error("Token is whitelisted and cannot be sold")]
    TokenWhitelisted,

    #[error("Zero SOL amount not allowed")]
    ZeroSolAmount,

    #[error("Splitter reference mismatch in sell config")]
    SellConfigSplitterMismatch,

    #[error("Too many approved swap programs (max 5)")]
    TooManyApprovedPrograms,

    #[error("Swap program not in approved list")]
    SwapProgramNotApproved,

    #[error("Destination must be native mint (SOL) or a whitelisted token")]
    DestinationNotAllowed,

    #[error("Swap did not decrease source token balance")]
    SwapSourceNotDecreased,

    #[error("Swap did not increase destination token balance")]
    SwapDestNotIncreased,

    #[error("Name exceeds maximum length (64 bytes)")]
    NameTooLong,
}

impl From<SplitterError> for ProgramError {
    fn from(e: SplitterError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
