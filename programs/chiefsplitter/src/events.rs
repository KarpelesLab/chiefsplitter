//! Structured binary log events emitted via sol_log_data

use solana_program::{log::sol_log_data, pubkey::Pubkey};

/// Discriminator prefixing Distribution events.
///
/// This is a fixed, arbitrary constant. It is NOT derived from a hash (in
/// particular it is not `sha256("event:Distribution")[..8]`), so indexers must
/// match on these literal bytes. It must never change: deployed clients rely
/// on it.
pub const DISTRIBUTION_DISCRIMINATOR: [u8; 8] = [0xa7, 0x3e, 0x5c, 0x1d, 0x8b, 0x4f, 0x2a, 0x69];

/// Emit a structured Distribution event.
///
/// Layout: 8 discriminator + 32 splitter + 32 recipient + 8 amount + 1 is_token
pub fn emit_distribution(splitter: &Pubkey, recipient: &Pubkey, amount: u64, is_token: bool) {
    let mut data = [0u8; 81];
    data[..8].copy_from_slice(&DISTRIBUTION_DISCRIMINATOR);
    data[8..40].copy_from_slice(splitter.as_ref());
    data[40..72].copy_from_slice(recipient.as_ref());
    data[72..80].copy_from_slice(&amount.to_le_bytes());
    data[80] = is_token as u8;
    sol_log_data(&[&data]);
}
