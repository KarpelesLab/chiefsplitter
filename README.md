# ChiefSplitter

A Solana program for configurable fee splitting. Anyone can create a splitter PDA that receives SOL or tokens and distributes them to configured recipients based on percentage shares.

## How It Works

1. **Create a splitter** -- anyone calls `CreateSplitter` with a nonce and a name to get a unique PDA
2. **Configure recipients** -- the admin sets addresses and shares (in 1/100 of percent, totaling 100%)
3. **Receive funds** -- send SOL or any SPL/Token2022 token to the splitter PDA
4. **Distribute** -- anyone can crank `DistributeSOL` or `DistributeToken` to split funds proportionally
5. **Auto-sell** -- optionally configure a token whitelist; non-whitelisted tokens can be swapped to SOL via approved DEXes (Jupiter, Raydium, etc.)

Shares use basis points: 10000 = 100%. Each recipient gets `floor(total * share / 10000)` per distribution. Rounding dust stays on the PDA until the next distribution.

## Features

- **Permissionless creation** -- anyone can create unlimited splitters (one PDA per creator+nonce)
- **SPL Token + Token 2022** -- distribute and swap both token standards
- **Recipient locking** -- recipients can lock their share to a guaranteed minimum rate
- **Admin controls** -- transfer admin, revoke admin (irreversible), configure distribution
- **Permissionless cranks** -- distribution and token swaps can be triggered by anyone
- **DEX integration** -- swap non-whitelisted tokens via CPI to admin-approved programs (Jupiter, Raydium, etc.) with automatic wSOL unwrap
- **Verifiable builds** -- CI produces reproducible binaries via `solana-verify`

## Program ID

```
ChiefYGYadRjMCgMNqbbFV8GUfiP2TqfRWzcWNynEoPh
```

## Instructions

| # | Instruction | Description | Access |
|---|-------------|-------------|--------|
| 0 | `CreateSplitter` | Create a new splitter PDA | Anyone (creator signs) |
| 1 | `SetSplitterDistribution` | Set recipient addresses and shares | Admin |
| 2 | `SetSplitterAdmin` | Transfer admin to a new address | Admin |
| 3 | `RevokeSplitterAdmin` | Revoke admin permanently (set to `11111...`) | Admin |
| 4 | `DistributeSOL` | Distribute SOL above rent-exempt to recipients | Permissionless |
| 5 | `DistributeToken` | Distribute tokens from splitter's ATA to recipients | Permissionless |
| 6 | `LockRecipient` | Lock a recipient's share to a guaranteed minimum | Recipient signs |
| 7 | `SetSplitterName` | Update the splitter's name (max 64 bytes UTF-8) | Admin |
| 8 | `SetSellConfig` | Configure token whitelist and approved swap programs | Admin |
| 9 | `CloseSellConfig` | Remove sell config, disable swapping | Admin |
| 10 | `SwapToken` | Swap non-whitelisted tokens via approved DEX CPI | Permissionless |
| 11 | `SnsProxy` | CPI proxy to the SNS Name Service / Name Offers programs, signed by the splitter PDA | Admin |

`#` is the Borsh enum tag (first byte of the instruction data).

## Accounts

### Splitter (507 bytes)

PDA: `["splitter", creator, nonce_le_bytes]`

| Field | Type | Description |
|-------|------|-------------|
| `discriminator` | `[u8; 8]` | Account type identifier |
| `creator` | `Pubkey` | Original creator (part of PDA seeds) |
| `admin` | `Pubkey` | Current admin (`Pubkey::default()` = revoked) |
| `nonce` | `u64` | PDA nonce |
| `bump` | `u8` | PDA bump |
| `num_recipients` | `u8` | Active recipient count (max 10) |
| `recipients` | `[Recipient; 10]` | Fixed array of recipients |
| `name_len` | `u8` | Actual byte length of `name` |
| `name` | `[u8; 64]` | UTF-8 name, zero-padded |

Each `Recipient` (36 bytes): `address: Pubkey`, `share: u16` (bps), `locked_share: u16` (minimum bps).

### SellConfig (523 bytes)

PDA: `["sell_config", splitter]`

| Field | Type | Description |
|-------|------|-------------|
| `discriminator` | `[u8; 8]` | Account type identifier |
| `splitter` | `Pubkey` | Back-reference to splitter |
| `num_whitelisted` | `u8` | Whitelisted mint count (max 10) |
| `whitelisted_mints` | `[Pubkey; 10]` | Tokens to keep (everything else can be swapped) |
| `num_approved_programs` | `u8` | Approved swap program count (max 5) |
| `approved_programs` | `[Pubkey; 5]` | DEX programs allowed for CPI swaps |
| `bump` | `u8` | PDA bump |

## Token Swap Flow (SwapToken)

1. Admin calls `SetSellConfig` with whitelisted mints and approved DEX program IDs
2. Non-whitelisted tokens accumulate in the splitter's ATAs
3. A cranker builds a swap instruction off-chain (e.g. via Jupiter SDK)
4. Cranker calls `SwapToken` passing the raw swap data and DEX accounts
5. Program validates: source not whitelisted, dest is wSOL or whitelisted, swap program approved
6. CPI to the DEX via `invoke_signed` (splitter PDA signs as authority)
7. Program verifies source balance decreased and dest balance increased
8. If dest is wSOL (native mint): auto-closes the account to unwrap to native SOL

The resulting SOL is distributed via the normal `DistributeSOL` crank.

## SNS Proxy (SnsProxy)

`SnsProxy` lets the splitter admin make the splitter PDA act on its own `.sol` domains (e.g. set a primary domain, update records). The instruction forwards raw instruction data and the remaining accounts via `invoke_signed`, with the splitter PDA as signer. It is admin-only (rejected once admin is revoked) and the target program must be one of:

- SNS Name Service: `namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX`
- SNS Name Offers: `85iDfUvr3HJyLM2zcq5BXSiDvUWfw6cSE1FfNBo8Ap29`

## Building

```bash
# Build for Solana
./scripts/build-sbf.sh

# Run unit tests
cargo test
```

## Testing

```bash
# Start local validator with program loaded
solana-test-validator \
  --upgradeable-program ChiefYGYadRjMCgMNqbbFV8GUfiP2TqfRWzcWNynEoPh \
  target/deploy/chiefsplitter.so ~/.config/solana/id.json \
  --reset &

# Run E2E tests
cd tests/typescript && npm ci && npm test
```

CI runs unit tests, E2E tests against a local validator, and a verifiable build on every push.

## Verification

```bash
cargo install solana-verify --version 0.4.11 --locked
solana-verify build --library-name chiefsplitter
solana-verify get-executable-hash target/deploy/chiefsplitter.so
```

## Project Structure

```
programs/chiefsplitter/src/
  lib.rs                  # Entrypoint, instruction enum, dispatch
  state.rs                # Account state (Splitter, SellConfig)
  error.rs                # Error types
  events.rs               # Structured binary log events
  instructions/
    create.rs             # CreateSplitter
    set_distribution.rs   # SetSplitterDistribution
    set_admin.rs          # SetSplitterAdmin
    revoke_admin.rs       # RevokeSplitterAdmin
    distribute_sol.rs     # DistributeSOL
    distribute_token.rs   # DistributeToken
    lock_recipient.rs     # LockRecipient
    set_name.rs           # SetSplitterName
    set_sell_config.rs    # SetSellConfig
    close_sell_config.rs  # CloseSellConfig
    swap_token.rs         # SwapToken (DEX CPI)
    sns_proxy.rs          # SnsProxy (SNS CPI)
scripts/
  build-sbf.sh            # Build script
  publish-idl.sh          # Publish the on-chain IDL (pinned program-metadata CLI)
tests/typescript/
  test_splitter.ts        # E2E tests
```

## License

MIT
