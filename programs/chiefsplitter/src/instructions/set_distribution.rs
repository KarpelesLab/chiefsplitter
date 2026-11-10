use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program_error::ProgramError,
    pubkey::Pubkey,
};

use crate::{
    error::SplitterError,
    state::{Recipient, Splitter, MAX_RECIPIENTS, TOTAL_SHARES},
};

/// Validate a proposed distribution against the existing one and build the new
/// recipients array.
///
/// Rules enforced:
/// - at most `MAX_RECIPIENTS` entries, no zero share, shares total `TOTAL_SHARES`
/// - no recipient may be `Pubkey::default()` (the unused-slot marker) or the
///   splitter account itself
/// - no address may appear more than once
/// - every existing recipient with `locked_share > 0` must still be present
///   with a share >= its locked share (a locked recipient cannot be reduced
///   below its lock, nor dropped from the list)
///
/// The `locked_share` of each surviving recipient is carried over unchanged.
pub fn build_new_distribution(
    splitter_key: &Pubkey,
    existing: &[Recipient],
    recipients: &[(Pubkey, u16)],
) -> Result<[Recipient; MAX_RECIPIENTS], ProgramError> {
    // Validate recipient count
    if recipients.len() > MAX_RECIPIENTS {
        return Err(SplitterError::TooManyRecipients.into());
    }

    // Validate total shares = 10000
    let total: u32 = recipients.iter().map(|(_, share)| *share as u32).sum();
    if total != TOTAL_SHARES as u32 {
        return Err(SplitterError::InvalidShareTotal.into());
    }

    for (i, (addr, share)) in recipients.iter().enumerate() {
        // Validate no zero shares
        if *share == 0 {
            return Err(SplitterError::ZeroShare.into());
        }

        // The default address marks unused slots and can never sign or receive
        if *addr == Pubkey::default() {
            msg!("Recipient {} is the default address", i);
            return Err(ProgramError::InvalidArgument);
        }

        // The splitter cannot pay itself
        if addr == splitter_key {
            msg!("Recipient {} is the splitter account itself", i);
            return Err(ProgramError::InvalidArgument);
        }

        // Each address may appear only once
        if recipients[..i].iter().any(|(other, _)| other == addr) {
            msg!("Duplicate recipient {}", addr);
            return Err(ProgramError::InvalidArgument);
        }
    }

    // Check locked share constraints from the point of view of the EXISTING
    // recipients: each locked recipient must still be present in the new list
    // with a share >= its locked share. Iterating over the new list only would
    // let the admin bypass a lock by simply omitting the recipient.
    //
    // Addresses are unique in the new list (checked above), so the share is
    // that of the single matching entry, or 0 when the recipient is absent.
    for locked in existing.iter().filter(|r| r.locked_share > 0) {
        let new_share = recipients
            .iter()
            .find(|(addr, _)| *addr == locked.address)
            .map(|(_, share)| *share)
            .unwrap_or(0);
        if new_share < locked.locked_share {
            msg!(
                "Recipient {} has locked share {} but new share is {}",
                locked.address,
                locked.locked_share,
                new_share
            );
            return Err(SplitterError::LockedShareViolation.into());
        }
    }

    // Build new recipients array, preserving locked_share for existing recipients.
    // If legacy state holds the same address more than once, keep the highest
    // lock (every one of them was validated above).
    let mut new_recipients = [Recipient::default(); MAX_RECIPIENTS];
    for (i, (addr, share)) in recipients.iter().enumerate() {
        let locked_share = existing
            .iter()
            .filter(|r| r.address == *addr)
            .map(|r| r.locked_share)
            .max()
            .unwrap_or(0);
        new_recipients[i] = Recipient {
            address: *addr,
            share: *share,
            locked_share,
        };
    }

    Ok(new_recipients)
}

/// Set the distribution configuration for a splitter
///
/// Accounts:
/// 0. `[writable]` Splitter
/// 1. `[signer]` Admin
pub fn process_set_distribution(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    recipients: &[(Pubkey, u16)],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let splitter_info = next_account_info(account_info_iter)?;
    let admin_info = next_account_info(account_info_iter)?;

    // Validate admin is signer
    if !admin_info.is_signer {
        return Err(SplitterError::MissingRequiredSigner.into());
    }

    // Defense in depth: the splitter must be owned by this program
    if splitter_info.owner != program_id {
        return Err(SplitterError::InvalidAccountOwner.into());
    }

    // Deserialize splitter
    let mut splitter = {
        let data = splitter_info.try_borrow_data()?;
        Splitter::try_from_slice(&data).map_err(|_| SplitterError::NotInitialized)?
    };

    if !splitter.is_initialized() {
        return Err(SplitterError::NotInitialized.into());
    }

    // Verify admin
    if splitter.is_admin_revoked() {
        return Err(SplitterError::AdminRevoked.into());
    }
    if *admin_info.key != splitter.admin {
        return Err(SplitterError::InvalidAuthority.into());
    }

    // Validate the new list (count, shares, addresses, locks) and rebuild
    let num_existing = (splitter.num_recipients as usize).min(MAX_RECIPIENTS);
    let new_recipients = build_new_distribution(
        splitter_info.key,
        &splitter.recipients[..num_existing],
        recipients,
    )?;

    splitter.num_recipients = recipients.len() as u8;
    splitter.recipients = new_recipients;

    // Serialize back
    let mut data = splitter_info.try_borrow_mut_data()?;
    splitter.serialize(&mut &mut data[..])?;

    msg!("Updated distribution with {} recipients", recipients.len());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rcpt(address: Pubkey, share: u16, locked_share: u16) -> Recipient {
        Recipient {
            address,
            share,
            locked_share,
        }
    }

    fn locked_violation() -> ProgramError {
        SplitterError::LockedShareViolation.into()
    }

    #[test]
    fn accepts_valid_distribution_and_preserves_locks() {
        let splitter = Pubkey::new_unique();
        let (a, b, c) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let existing = [rcpt(a, 5000, 4000), rcpt(b, 5000, 0)];

        // Reordered, a kept above its lock, c added
        let out = build_new_distribution(&splitter, &existing, &[(c, 3000), (a, 4000), (b, 3000)])
            .unwrap();

        assert_eq!(out[0].address, c);
        assert_eq!((out[0].share, out[0].locked_share), (3000, 0));
        assert_eq!(out[1].address, a);
        assert_eq!((out[1].share, out[1].locked_share), (4000, 4000));
        assert_eq!(out[2].address, b);
        assert_eq!((out[2].share, out[2].locked_share), (3000, 0));
        for slot in &out[3..] {
            assert_eq!(slot.address, Pubkey::default());
            assert_eq!((slot.share, slot.locked_share), (0, 0));
        }
    }

    #[test]
    fn rejects_dropping_locked_recipient() {
        let splitter = Pubkey::new_unique();
        let (a, b, c) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let existing = [rcpt(a, 5000, 5000), rcpt(b, 5000, 0)];

        // Finding #1: omit the locked recipient entirely
        assert_eq!(
            build_new_distribution(&splitter, &existing, &[(b, 10000)]).unwrap_err(),
            locked_violation()
        );
        assert_eq!(
            build_new_distribution(&splitter, &existing, &[(b, 5000), (c, 5000)]).unwrap_err(),
            locked_violation()
        );
    }

    #[test]
    fn rejects_reducing_locked_recipient() {
        let splitter = Pubkey::new_unique();
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
        let existing = [rcpt(a, 5000, 5000), rcpt(b, 5000, 0)];

        assert_eq!(
            build_new_distribution(&splitter, &existing, &[(a, 4999), (b, 5001)]).unwrap_err(),
            locked_violation()
        );
        // Exactly at the lock and above it are fine
        assert!(build_new_distribution(&splitter, &existing, &[(a, 5000), (b, 5000)]).is_ok());
        assert!(build_new_distribution(&splitter, &existing, &[(a, 10000)]).is_ok());
    }

    #[test]
    fn unlocked_recipient_can_be_dropped() {
        let splitter = Pubkey::new_unique();
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
        let existing = [rcpt(a, 10000, 0)];

        let out = build_new_distribution(&splitter, &existing, &[(b, 10000)]).unwrap();
        assert_eq!(out[0].address, b);
        assert_eq!(out[0].locked_share, 0);
    }

    #[test]
    fn rejects_duplicate_addresses() {
        let splitter = Pubkey::new_unique();
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());

        assert_eq!(
            build_new_distribution(&splitter, &[], &[(a, 5000), (b, 2000), (a, 3000)]).unwrap_err(),
            ProgramError::InvalidArgument
        );
        // Splitting a locked recipient across two entries is rejected too
        let existing = [rcpt(a, 10000, 6000)];
        assert_eq!(
            build_new_distribution(&splitter, &existing, &[(a, 3000), (a, 3000), (b, 4000)])
                .unwrap_err(),
            ProgramError::InvalidArgument
        );
    }

    #[test]
    fn rejects_default_and_self_addresses() {
        let splitter = Pubkey::new_unique();
        let a = Pubkey::new_unique();

        assert_eq!(
            build_new_distribution(&splitter, &[], &[(a, 5000), (Pubkey::default(), 5000)])
                .unwrap_err(),
            ProgramError::InvalidArgument
        );
        assert_eq!(
            build_new_distribution(&splitter, &[], &[(splitter, 5000), (a, 5000)]).unwrap_err(),
            ProgramError::InvalidArgument
        );
    }

    #[test]
    fn rejects_bad_counts_and_shares() {
        let splitter = Pubkey::new_unique();
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();

        assert_eq!(
            build_new_distribution(&splitter, &[], &[]).unwrap_err(),
            SplitterError::InvalidShareTotal.into()
        );
        assert_eq!(
            build_new_distribution(&splitter, &[], &[(a, 9999)]).unwrap_err(),
            SplitterError::InvalidShareTotal.into()
        );
        assert_eq!(
            build_new_distribution(&splitter, &[], &[(a, 10000), (b, 0)]).unwrap_err(),
            SplitterError::ZeroShare.into()
        );
        // u16 shares cannot overflow the u32 total
        assert_eq!(
            build_new_distribution(&splitter, &[], &[(a, u16::MAX), (b, u16::MAX)]).unwrap_err(),
            SplitterError::InvalidShareTotal.into()
        );

        let too_many: Vec<(Pubkey, u16)> = (0..MAX_RECIPIENTS + 1)
            .map(|_| (Pubkey::new_unique(), 1))
            .collect();
        assert_eq!(
            build_new_distribution(&splitter, &[], &too_many).unwrap_err(),
            SplitterError::TooManyRecipients.into()
        );

        let max: Vec<(Pubkey, u16)> = (0..MAX_RECIPIENTS)
            .map(|_| (Pubkey::new_unique(), TOTAL_SHARES / MAX_RECIPIENTS as u16))
            .collect();
        assert!(build_new_distribution(&splitter, &[], &max).is_ok());
    }

    #[test]
    fn legacy_duplicate_existing_entries_keep_highest_lock() {
        // State written before duplicates were rejected may hold the same
        // address twice; LockRecipient only ever locked the first match.
        let splitter = Pubkey::new_unique();
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
        let existing = [rcpt(a, 3000, 3000), rcpt(a, 3000, 0), rcpt(b, 4000, 0)];

        assert_eq!(
            build_new_distribution(&splitter, &existing, &[(a, 2999), (b, 7001)]).unwrap_err(),
            locked_violation()
        );
        let out = build_new_distribution(&splitter, &existing, &[(b, 7000), (a, 3000)]).unwrap();
        assert_eq!(out[1].address, a);
        assert_eq!(out[1].locked_share, 3000);
    }

    fn run_set_distribution(
        splitter: &Splitter,
        owner: &Pubkey,
        admin: &Pubkey,
        recipients: &[(Pubkey, u16)],
    ) -> (ProgramResult, Splitter) {
        let program_id = crate::id();
        let splitter_key = Pubkey::new_unique();
        let system = Pubkey::default();
        let mut data = borsh::to_vec(splitter).unwrap();
        let (mut lamports, mut admin_lamports) = (1_000_000u64, 1_000_000u64);
        let mut admin_data: [u8; 0] = [];
        let result = {
            let accounts = [
                AccountInfo::new(
                    &splitter_key,
                    false,
                    true,
                    &mut lamports,
                    &mut data,
                    owner,
                    false,
                    0,
                ),
                AccountInfo::new(
                    admin,
                    true,
                    false,
                    &mut admin_lamports,
                    &mut admin_data,
                    &system,
                    false,
                    0,
                ),
            ];
            process_set_distribution(&program_id, &accounts, recipients)
        };
        (result, Splitter::try_from_slice(&data).unwrap())
    }

    #[test]
    fn process_rejects_dropping_locked_recipient_and_leaves_state_untouched() {
        let admin = Pubkey::new_unique();
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
        let mut splitter = Splitter::new(admin, 0, 255, b"t");
        splitter.num_recipients = 2;
        splitter.recipients[0] = rcpt(a, 5000, 5000);
        splitter.recipients[1] = rcpt(b, 5000, 0);

        let (res, after) = run_set_distribution(&splitter, &crate::id(), &admin, &[(b, 10000)]);
        assert_eq!(res, Err(locked_violation()));
        assert_eq!(after.num_recipients, 2);
        assert_eq!(after.recipients[0].address, a);
        assert_eq!(after.recipients[0].locked_share, 5000);

        // A compliant update goes through and keeps the lock
        let (res, after) =
            run_set_distribution(&splitter, &crate::id(), &admin, &[(b, 4000), (a, 6000)]);
        assert_eq!(res, Ok(()));
        assert_eq!(after.num_recipients, 2);
        assert_eq!(after.recipients[1].address, a);
        assert_eq!(after.recipients[1].share, 6000);
        assert_eq!(after.recipients[1].locked_share, 5000);
    }

    #[test]
    fn process_rejects_splitter_not_owned_by_program() {
        let admin = Pubkey::new_unique();
        let splitter = Splitter::new(admin, 0, 255, b"t");

        let (res, after) = run_set_distribution(
            &splitter,
            &Pubkey::new_unique(),
            &admin,
            &[(Pubkey::new_unique(), 10000)],
        );
        assert_eq!(res, Err(SplitterError::InvalidAccountOwner.into()));
        assert_eq!(after.recipients[0].address, admin);
    }
}
