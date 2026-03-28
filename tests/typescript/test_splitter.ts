import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  createMint,
  getOrCreateAssociatedTokenAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
} from "@solana/spl-token";
import * as borsh from "borsh";
import * as fs from "fs";
import * as path from "path";

const PROGRAM_ID = new PublicKey(
  "ChiefYGYadRjMCgMNqbbFV8GUfiP2TqfRWzcWNynEoPh"
);

const SPLITTER_SEED = Buffer.from("splitter");

// ---- Borsh helpers ----

function encodeCreateSplitter(nonce: bigint): Buffer {
  // variant 0 + u64 nonce
  const buf = Buffer.alloc(1 + 8);
  buf.writeUInt8(0, 0);
  buf.writeBigUInt64LE(nonce, 1);
  return buf;
}

function encodeSetDistribution(
  recipients: { address: PublicKey; share: number }[]
): Buffer {
  // variant 1 + vec length (u32) + entries (32 + 2 each)
  const vecLen = recipients.length;
  const buf = Buffer.alloc(1 + 4 + vecLen * (32 + 2));
  let offset = 0;
  buf.writeUInt8(1, offset);
  offset += 1;
  buf.writeUInt32LE(vecLen, offset);
  offset += 4;
  for (const r of recipients) {
    r.address.toBuffer().copy(buf, offset);
    offset += 32;
    buf.writeUInt16LE(r.share, offset);
    offset += 2;
  }
  return buf;
}

function encodeSetAdmin(newAdmin: PublicKey): Buffer {
  // variant 2 + pubkey
  const buf = Buffer.alloc(1 + 32);
  buf.writeUInt8(2, 0);
  newAdmin.toBuffer().copy(buf, 1);
  return buf;
}

function encodeRevokeAdmin(): Buffer {
  return Buffer.from([3]);
}

function encodeDistributeSOL(): Buffer {
  return Buffer.from([4]);
}

function encodeDistributeToken(): Buffer {
  return Buffer.from([5]);
}

function encodeLockRecipient(minShare: number): Buffer {
  // variant 6 + u16
  const buf = Buffer.alloc(1 + 2);
  buf.writeUInt8(6, 0);
  buf.writeUInt16LE(minShare, 1);
  return buf;
}

function encodeSetSellConfig(
  mints: PublicKey[],
  approvedPrograms: PublicKey[]
): Buffer {
  // variant 7 + vec<Pubkey> whitelisted_mints + vec<Pubkey> approved_swap_programs
  const totalSize =
    1 + 4 + mints.length * 32 + 4 + approvedPrograms.length * 32;
  const buf = Buffer.alloc(totalSize);
  let offset = 0;
  buf.writeUInt8(7, offset);
  offset += 1;
  buf.writeUInt32LE(mints.length, offset);
  offset += 4;
  for (const m of mints) {
    m.toBuffer().copy(buf, offset);
    offset += 32;
  }
  buf.writeUInt32LE(approvedPrograms.length, offset);
  offset += 4;
  for (const p of approvedPrograms) {
    p.toBuffer().copy(buf, offset);
    offset += 32;
  }
  return buf;
}

function encodeCloseSellConfig(): Buffer {
  return Buffer.from([8]);
}

function encodeSwapToken(swapData: Buffer): Buffer {
  // variant 9 + vec<u8> swap_data
  const buf = Buffer.alloc(1 + 4 + swapData.length);
  buf.writeUInt8(9, 0);
  buf.writeUInt32LE(swapData.length, 1);
  swapData.copy(buf, 5);
  return buf;
}

const SELL_CONFIG_SEED = Buffer.from("sell_config");

function findSellConfigPDA(splitter: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [SELL_CONFIG_SEED, splitter.toBuffer()],
    PROGRAM_ID
  );
}

// ---- PDA helpers ----

function findSplitterPDA(
  creator: PublicKey,
  nonce: bigint
): [PublicKey, number] {
  const nonceBuf = Buffer.alloc(8);
  nonceBuf.writeBigUInt64LE(nonce);
  return PublicKey.findProgramAddressSync(
    [SPLITTER_SEED, creator.toBuffer(), nonceBuf],
    PROGRAM_ID
  );
}

// ---- Test runner ----

async function main() {
  const connection = new Connection("http://localhost:8899", "confirmed");

  // Load payer from default keypair
  const keypairPath =
    process.env.KEYPAIR_PATH ||
    path.join(
      process.env.HOME || "~",
      ".config",
      "solana",
      "id.json"
    );
  const secret = JSON.parse(fs.readFileSync(keypairPath, "utf-8"));
  const payer = Keypair.fromSecretKey(Uint8Array.from(secret));

  // Airdrop SOL to payer
  console.log("Airdropping SOL to payer...");
  const sig = await connection.requestAirdrop(
    payer.publicKey,
    10 * LAMPORTS_PER_SOL
  );
  await connection.confirmTransaction(sig);

  let passed = 0;
  let failed = 0;

  async function test(name: string, fn: () => Promise<void>) {
    try {
      await fn();
      console.log(`  ✓ ${name}`);
      passed++;
    } catch (e: any) {
      console.error(`  ✗ ${name}: ${e.message || e}`);
      failed++;
    }
  }

  // Generate recipient wallets
  const recipient1 = Keypair.generate();
  const recipient2 = Keypair.generate();
  const recipient3 = Keypair.generate();

  // Fund recipients so they can receive SOL (need to exist on-chain)
  for (const r of [recipient1, recipient2, recipient3]) {
    const rsig = await connection.requestAirdrop(
      r.publicKey,
      0.01 * LAMPORTS_PER_SOL
    );
    await connection.confirmTransaction(rsig);
  }

  const nonce = BigInt(0);
  const [splitterPDA, _bump] = findSplitterPDA(payer.publicKey, nonce);

  console.log("\n=== ChiefSplitter E2E Tests ===\n");
  console.log(`Program ID: ${PROGRAM_ID}`);
  console.log(`Payer: ${payer.publicKey}`);
  console.log(`Splitter PDA: ${splitterPDA}\n`);

  // Test 1: Create splitter
  await test("CreateSplitter", async () => {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },
        {
          pubkey: SystemProgram.programId,
          isSigner: false,
          isWritable: false,
        },
      ],
      data: encodeCreateSplitter(nonce),
    });
    const tx = new Transaction().add(ix);
    await sendAndConfirmTransaction(connection, tx, [payer]);

    const info = await connection.getAccountInfo(splitterPDA);
    if (!info) throw new Error("Splitter account not created");
    if (!info.data || info.data.length === 0)
      throw new Error("Splitter account has no data");
  });

  // Test 2: Set distribution
  await test("SetSplitterDistribution", async () => {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
      ],
      data: encodeSetDistribution([
        { address: recipient1.publicKey, share: 5000 },
        { address: recipient2.publicKey, share: 3000 },
        { address: recipient3.publicKey, share: 2000 },
      ]),
    });
    const tx = new Transaction().add(ix);
    await sendAndConfirmTransaction(connection, tx, [payer]);
  });

  // Test 3: Distribute SOL
  await test("DistributeSOL", async () => {
    // Send 1 SOL to the splitter PDA
    const transferIx = SystemProgram.transfer({
      fromPubkey: payer.publicKey,
      toPubkey: splitterPDA,
      lamports: 1 * LAMPORTS_PER_SOL,
    });
    const transferTx = new Transaction().add(transferIx);
    await sendAndConfirmTransaction(connection, transferTx, [payer]);

    // Record balances before
    const r1Before = await connection.getBalance(recipient1.publicKey);
    const r2Before = await connection.getBalance(recipient2.publicKey);
    const r3Before = await connection.getBalance(recipient3.publicKey);

    // Distribute
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        {
          pubkey: recipient1.publicKey,
          isSigner: false,
          isWritable: true,
        },
        {
          pubkey: recipient2.publicKey,
          isSigner: false,
          isWritable: true,
        },
        {
          pubkey: recipient3.publicKey,
          isSigner: false,
          isWritable: true,
        },
      ],
      data: encodeDistributeSOL(),
    });
    const tx = new Transaction().add(ix);
    await sendAndConfirmTransaction(connection, tx, [payer]);

    // Check balances after
    const r1After = await connection.getBalance(recipient1.publicKey);
    const r2After = await connection.getBalance(recipient2.publicKey);
    const r3After = await connection.getBalance(recipient3.publicKey);

    const r1Received = r1After - r1Before;
    const r2Received = r2After - r2Before;
    const r3Received = r3After - r3Before;

    console.log(
      `    Recipient1: +${r1Received} lamports (50%)`
    );
    console.log(
      `    Recipient2: +${r2Received} lamports (30%)`
    );
    console.log(
      `    Recipient3: +${r3Received} lamports (20%)`
    );

    // Verify approximate percentages (allow small rounding)
    const total = r1Received + r2Received + r3Received;
    if (total < 0.99 * LAMPORTS_PER_SOL)
      throw new Error(`Total distributed too low: ${total}`);
    if (Math.abs(r1Received / total - 0.5) > 0.01)
      throw new Error(`Recipient1 share wrong`);
    if (Math.abs(r2Received / total - 0.3) > 0.01)
      throw new Error(`Recipient2 share wrong`);
    if (Math.abs(r3Received / total - 0.2) > 0.01)
      throw new Error(`Recipient3 share wrong`);
  });

  // Test 4: Lock recipient
  await test("LockRecipient", async () => {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        {
          pubkey: recipient1.publicKey,
          isSigner: true,
          isWritable: false,
        },
      ],
      data: encodeLockRecipient(5000),
    });
    const tx = new Transaction().add(ix);
    await sendAndConfirmTransaction(connection, tx, [payer, recipient1]);
  });

  // Test 5: Cannot reduce locked recipient below locked share
  await test(
    "SetDistribution rejects reducing locked recipient",
    async () => {
      const ix = new TransactionInstruction({
        programId: PROGRAM_ID,
        keys: [
          { pubkey: splitterPDA, isSigner: false, isWritable: true },
          {
            pubkey: payer.publicKey,
            isSigner: true,
            isWritable: false,
          },
        ],
        data: encodeSetDistribution([
          { address: recipient1.publicKey, share: 3000 }, // Below locked 5000
          { address: recipient2.publicKey, share: 4000 },
          { address: recipient3.publicKey, share: 3000 },
        ]),
      });
      const tx = new Transaction().add(ix);
      try {
        await sendAndConfirmTransaction(connection, tx, [payer]);
        throw new Error("Should have failed");
      } catch (e: any) {
        if (e.message === "Should have failed") throw e;
        // Expected failure
      }
    }
  );

  // Test 6: Can increase locked recipient
  await test("SetDistribution allows increasing locked recipient", async () => {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
      ],
      data: encodeSetDistribution([
        { address: recipient1.publicKey, share: 6000 },
        { address: recipient2.publicKey, share: 2000 },
        { address: recipient3.publicKey, share: 2000 },
      ]),
    });
    const tx = new Transaction().add(ix);
    await sendAndConfirmTransaction(connection, tx, [payer]);
  });

  // Test 7: Set admin
  const newAdmin = Keypair.generate();
  const adminSig = await connection.requestAirdrop(
    newAdmin.publicKey,
    0.1 * LAMPORTS_PER_SOL
  );
  await connection.confirmTransaction(adminSig);

  await test("SetSplitterAdmin", async () => {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
      ],
      data: encodeSetAdmin(newAdmin.publicKey),
    });
    const tx = new Transaction().add(ix);
    await sendAndConfirmTransaction(connection, tx, [payer]);
  });

  // Test 8: Old admin can no longer modify
  await test("Old admin rejected after transfer", async () => {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
      ],
      data: encodeSetDistribution([
        { address: recipient1.publicKey, share: 6000 },
        { address: recipient2.publicKey, share: 2000 },
        { address: recipient3.publicKey, share: 2000 },
      ]),
    });
    const tx = new Transaction().add(ix);
    try {
      await sendAndConfirmTransaction(connection, tx, [payer]);
      throw new Error("Should have failed");
    } catch (e: any) {
      if (e.message === "Should have failed") throw e;
    }
  });

  // Test 9: Transfer admin back and revoke
  await test("RevokeSplitterAdmin", async () => {
    // Transfer back to payer first
    const ix1 = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        {
          pubkey: newAdmin.publicKey,
          isSigner: true,
          isWritable: false,
        },
      ],
      data: encodeSetAdmin(payer.publicKey),
    });
    const tx1 = new Transaction().add(ix1);
    await sendAndConfirmTransaction(connection, tx1, [payer, newAdmin]);

    // Now revoke
    const ix2 = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
      ],
      data: encodeRevokeAdmin(),
    });
    const tx2 = new Transaction().add(ix2);
    await sendAndConfirmTransaction(connection, tx2, [payer]);
  });

  // Test 10: Admin revoked - cannot modify
  await test("Admin revoked prevents modifications", async () => {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: splitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
      ],
      data: encodeSetDistribution([
        { address: recipient1.publicKey, share: 6000 },
        { address: recipient2.publicKey, share: 2000 },
        { address: recipient3.publicKey, share: 2000 },
      ]),
    });
    const tx = new Transaction().add(ix);
    try {
      await sendAndConfirmTransaction(connection, tx, [payer]);
      throw new Error("Should have failed");
    } catch (e: any) {
      if (e.message === "Should have failed") throw e;
    }
  });

  // Test 11: Distribute SPL tokens
  await test("DistributeToken (SPL Token)", async () => {
    // Create a second splitter for token testing
    const tokenNonce = BigInt(1);
    const [tokenSplitterPDA] = findSplitterPDA(
      payer.publicKey,
      tokenNonce
    );

    // Create splitter
    const createIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: tokenSplitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },
        {
          pubkey: SystemProgram.programId,
          isSigner: false,
          isWritable: false,
        },
      ],
      data: encodeCreateSplitter(tokenNonce),
    });
    const createTx = new Transaction().add(createIx);
    await sendAndConfirmTransaction(connection, createTx, [payer]);

    // Set distribution
    const distIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: tokenSplitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
      ],
      data: encodeSetDistribution([
        { address: recipient1.publicKey, share: 5000 },
        { address: recipient2.publicKey, share: 3000 },
        { address: recipient3.publicKey, share: 2000 },
      ]),
    });
    const distTx = new Transaction().add(distIx);
    await sendAndConfirmTransaction(connection, distTx, [payer]);

    // Create an SPL token mint
    const mint = await createMint(
      connection,
      payer,
      payer.publicKey,
      null,
      6, // 6 decimals
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    // Create token accounts
    const splitterATA = await getOrCreateAssociatedTokenAccount(
      connection,
      payer,
      mint,
      tokenSplitterPDA,
      true // allowOwnerOffCurve for PDA
    );

    const r1ATA = await getOrCreateAssociatedTokenAccount(
      connection,
      payer,
      mint,
      recipient1.publicKey
    );
    const r2ATA = await getOrCreateAssociatedTokenAccount(
      connection,
      payer,
      mint,
      recipient2.publicKey
    );
    const r3ATA = await getOrCreateAssociatedTokenAccount(
      connection,
      payer,
      mint,
      recipient3.publicKey
    );

    // Mint tokens to the splitter's token account
    const mintAmount = 1_000_000_000; // 1000 tokens with 6 decimals
    await mintTo(
      connection,
      payer,
      mint,
      splitterATA.address,
      payer,
      mintAmount
    );

    // Distribute tokens
    const tokenDistIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        {
          pubkey: tokenSplitterPDA,
          isSigner: false,
          isWritable: false,
        },
        { pubkey: mint, isSigner: false, isWritable: false },
        {
          pubkey: splitterATA.address,
          isSigner: false,
          isWritable: true,
        },
        {
          pubkey: TOKEN_PROGRAM_ID,
          isSigner: false,
          isWritable: false,
        },
        { pubkey: r1ATA.address, isSigner: false, isWritable: true },
        { pubkey: r2ATA.address, isSigner: false, isWritable: true },
        { pubkey: r3ATA.address, isSigner: false, isWritable: true },
      ],
      data: encodeDistributeToken(),
    });
    const tokenDistTx = new Transaction().add(tokenDistIx);
    await sendAndConfirmTransaction(connection, tokenDistTx, [payer]);

    // Verify balances
    const r1Balance = (await getAccount(connection, r1ATA.address))
      .amount;
    const r2Balance = (await getAccount(connection, r2ATA.address))
      .amount;
    const r3Balance = (await getAccount(connection, r3ATA.address))
      .amount;

    console.log(`    Recipient1: ${r1Balance} tokens (50%)`);
    console.log(`    Recipient2: ${r2Balance} tokens (30%)`);
    console.log(`    Recipient3: ${r3Balance} tokens (20%)`);

    const totalDistributed = r1Balance + r2Balance + r3Balance;
    if (totalDistributed !== BigInt(mintAmount))
      throw new Error(
        `Total distributed ${totalDistributed} != ${mintAmount}`
      );
    if (r1Balance !== BigInt(500_000_000))
      throw new Error(`R1 got ${r1Balance}, expected 500000000`);
    if (r2Balance !== BigInt(300_000_000))
      throw new Error(`R2 got ${r2Balance}, expected 300000000`);
    if (r3Balance !== BigInt(200_000_000))
      throw new Error(`R3 got ${r3Balance}, expected 200000000`);
  });

  // Test 12: Distribute Token2022 tokens
  await test("DistributeToken (Token 2022)", async () => {
    const t22Nonce = BigInt(2);
    const [t22SplitterPDA] = findSplitterPDA(payer.publicKey, t22Nonce);

    // Create splitter
    const createIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: t22SplitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },
        {
          pubkey: SystemProgram.programId,
          isSigner: false,
          isWritable: false,
        },
      ],
      data: encodeCreateSplitter(t22Nonce),
    });
    const createTx = new Transaction().add(createIx);
    await sendAndConfirmTransaction(connection, createTx, [payer]);

    // Set distribution
    const distIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: t22SplitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
      ],
      data: encodeSetDistribution([
        { address: recipient1.publicKey, share: 4000 },
        { address: recipient2.publicKey, share: 6000 },
      ]),
    });
    const distTx = new Transaction().add(distIx);
    await sendAndConfirmTransaction(connection, distTx, [payer]);

    // Create a Token2022 mint
    const mint22 = await createMint(
      connection,
      payer,
      payer.publicKey,
      null,
      9, // 9 decimals
      undefined,
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    // Create token accounts
    const splitter22ATA = await getOrCreateAssociatedTokenAccount(
      connection,
      payer,
      mint22,
      t22SplitterPDA,
      true,
      undefined,
      undefined,
      TOKEN_2022_PROGRAM_ID
    );
    const r1ATA22 = await getOrCreateAssociatedTokenAccount(
      connection,
      payer,
      mint22,
      recipient1.publicKey,
      false,
      undefined,
      undefined,
      TOKEN_2022_PROGRAM_ID
    );
    const r2ATA22 = await getOrCreateAssociatedTokenAccount(
      connection,
      payer,
      mint22,
      recipient2.publicKey,
      false,
      undefined,
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    // Mint tokens
    const mintAmount22 = 5_000_000_000_000n; // 5000 tokens (9 decimals)
    await mintTo(
      connection,
      payer,
      mint22,
      splitter22ATA.address,
      payer,
      mintAmount22,
      undefined,
      undefined,
      TOKEN_2022_PROGRAM_ID
    );

    // Distribute
    const tokenDistIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        {
          pubkey: t22SplitterPDA,
          isSigner: false,
          isWritable: false,
        },
        { pubkey: mint22, isSigner: false, isWritable: false },
        {
          pubkey: splitter22ATA.address,
          isSigner: false,
          isWritable: true,
        },
        {
          pubkey: TOKEN_2022_PROGRAM_ID,
          isSigner: false,
          isWritable: false,
        },
        { pubkey: r1ATA22.address, isSigner: false, isWritable: true },
        { pubkey: r2ATA22.address, isSigner: false, isWritable: true },
      ],
      data: encodeDistributeToken(),
    });
    const tokenDistTx = new Transaction().add(tokenDistIx);
    await sendAndConfirmTransaction(connection, tokenDistTx, [payer]);

    const r1Bal = (await getAccount(connection, r1ATA22.address, undefined, TOKEN_2022_PROGRAM_ID)).amount;
    const r2Bal = (await getAccount(connection, r2ATA22.address, undefined, TOKEN_2022_PROGRAM_ID)).amount;

    console.log(`    Recipient1: ${r1Bal} tokens (40%)`);
    console.log(`    Recipient2: ${r2Bal} tokens (60%)`);

    if (r1Bal + r2Bal !== mintAmount22)
      throw new Error(`Total mismatch: ${r1Bal + r2Bal} != ${mintAmount22}`);
    if (r1Bal !== 2_000_000_000_000n)
      throw new Error(`R1 got ${r1Bal}, expected 2000000000000`);
    if (r2Bal !== 3_000_000_000_000n)
      throw new Error(`R2 got ${r2Bal}, expected 3000000000000`);
  });

  // Test 13: SetSellConfig with approved programs
  await test("SetSellConfig (whitelist + approved programs)", async () => {
    const sellNonce = BigInt(3);
    const [sellSplitterPDA] = findSplitterPDA(payer.publicKey, sellNonce);
    const [sellConfigPDA] = findSellConfigPDA(sellSplitterPDA);

    // Create splitter
    const createIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: sellSplitterPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data: encodeCreateSplitter(sellNonce),
    });
    await sendAndConfirmTransaction(connection, new Transaction().add(createIx), [payer]);

    // A fake "Jupiter" program ID for testing
    const fakeJupiter = Keypair.generate().publicKey;
    const goodMint = await createMint(
      connection, payer, payer.publicKey, null, 6, undefined, undefined, TOKEN_PROGRAM_ID
    );

    const setSellIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: sellSplitterPDA, isSigner: false, isWritable: false },
        { pubkey: sellConfigPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: false },
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data: encodeSetSellConfig([goodMint], [fakeJupiter]),
    });
    await sendAndConfirmTransaction(connection, new Transaction().add(setSellIx), [payer]);

    const info = await connection.getAccountInfo(sellConfigPDA);
    if (!info || info.data.length === 0)
      throw new Error("SellConfig not created");
  });

  // Test 14: SwapToken rejects non-approved swap program
  await test("SwapToken rejects non-approved program", async () => {
    const sellNonce = BigInt(3);
    const [sellSplitterPDA] = findSplitterPDA(payer.publicKey, sellNonce);
    const [sellConfigPDA] = findSellConfigPDA(sellSplitterPDA);

    const junkMint = await createMint(
      connection, payer, payer.publicKey, null, 6, undefined, undefined, TOKEN_PROGRAM_ID
    );
    const splitterJunkATA = await getOrCreateAssociatedTokenAccount(
      connection, payer, junkMint, sellSplitterPDA, true
    );
    await mintTo(connection, payer, junkMint, splitterJunkATA.address, payer, 1_000_000);

    // Use system program as "swap program" - not approved
    const fakeDestMint = await createMint(
      connection, payer, payer.publicKey, null, 6, undefined, undefined, TOKEN_PROGRAM_ID
    );
    const splitterDestATA = await getOrCreateAssociatedTokenAccount(
      connection, payer, fakeDestMint, sellSplitterPDA, true
    );

    const swapIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: sellSplitterPDA, isSigner: false, isWritable: true },
        { pubkey: sellConfigPDA, isSigner: false, isWritable: false },
        { pubkey: junkMint, isSigner: false, isWritable: false },
        { pubkey: splitterJunkATA.address, isSigner: false, isWritable: true },
        { pubkey: splitterDestATA.address, isSigner: false, isWritable: true },
        { pubkey: fakeDestMint, isSigner: false, isWritable: false },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false }, // not approved!
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      ],
      data: encodeSwapToken(Buffer.from([0])),
    });
    try {
      await sendAndConfirmTransaction(connection, new Transaction().add(swapIx), [payer]);
      throw new Error("Should have failed");
    } catch (e: any) {
      if (e.message === "Should have failed") throw e;
      // Expected: SwapProgramNotApproved
    }
  });

  // Test 15: SwapToken rejects whitelisted source token
  await test("SwapToken rejects whitelisted source token", async () => {
    const sellNonce = BigInt(3);
    const [sellSplitterPDA] = findSplitterPDA(payer.publicKey, sellNonce);
    const [sellConfigPDA] = findSellConfigPDA(sellSplitterPDA);

    // Read the sell config to get the whitelisted mint
    const configInfo = await connection.getAccountInfo(sellConfigPDA);
    if (!configInfo) throw new Error("SellConfig missing");
    // The whitelisted mint starts at offset 8+32+1 = 41
    const whitelistedMint = new PublicKey(configInfo.data.subarray(41, 73));

    const splitterGoodATA = await getOrCreateAssociatedTokenAccount(
      connection, payer, whitelistedMint, sellSplitterPDA, true
    );
    await mintTo(connection, payer, whitelistedMint, splitterGoodATA.address, payer, 1_000_000);

    const fakeDestMint = Keypair.generate().publicKey;
    const fakeJupiter = Keypair.generate().publicKey;

    const swapIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: sellSplitterPDA, isSigner: false, isWritable: true },
        { pubkey: sellConfigPDA, isSigner: false, isWritable: false },
        { pubkey: whitelistedMint, isSigner: false, isWritable: false }, // whitelisted!
        { pubkey: splitterGoodATA.address, isSigner: false, isWritable: true },
        { pubkey: splitterGoodATA.address, isSigner: false, isWritable: true },
        { pubkey: fakeDestMint, isSigner: false, isWritable: false },
        { pubkey: fakeJupiter, isSigner: false, isWritable: false },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      ],
      data: encodeSwapToken(Buffer.from([0])),
    });
    try {
      await sendAndConfirmTransaction(connection, new Transaction().add(swapIx), [payer]);
      throw new Error("Should have failed");
    } catch (e: any) {
      if (e.message === "Should have failed") throw e;
      // Expected: TokenWhitelisted
    }
  });

  // Test 16: CloseSellConfig
  await test("CloseSellConfig disables selling", async () => {
    const sellNonce = BigInt(3);
    const [sellSplitterPDA] = findSplitterPDA(payer.publicKey, sellNonce);
    const [sellConfigPDA] = findSellConfigPDA(sellSplitterPDA);

    const closeIx = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: sellSplitterPDA, isSigner: false, isWritable: false },
        { pubkey: sellConfigPDA, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      ],
      data: encodeCloseSellConfig(),
    });
    await sendAndConfirmTransaction(connection, new Transaction().add(closeIx), [payer]);

    const info = await connection.getAccountInfo(sellConfigPDA);
    if (info !== null && info.lamports > 0)
      throw new Error("SellConfig account should be closed");
  });

  // Summary
  console.log(`\n=== Results: ${passed} passed, ${failed} failed ===\n`);
  if (failed > 0) process.exit(1);
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
