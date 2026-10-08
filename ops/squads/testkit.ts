/** Helpers for test-localnet.sh only (not part of the migration tool). */
import * as m from "@sqds/multisig";
import { Connection, Keypair, PublicKey, SystemProgram, Transaction, TransactionInstruction, TransactionMessage, sendAndConfirmTransaction } from "@solana/web3.js";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const kp = (p: string) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(p, "utf8"))));

async function singleApprovalIsNotEnough(): Promise<void> {
  const r = JSON.parse(readFileSync(process.env.RECORD!, "utf8"));
  const c = new Connection(process.env.RPC_URL!, "confirmed");
  const payer = kp(process.env.PAYER!);
  const m1 = kp(`${process.env.KEYS_DIR}/member-1.json`);
  const multisigPda = new PublicKey(r.multisig);
  const vault = new PublicKey(r.vault);
  const ms = await m.accounts.Multisig.fromAccountAddress(c, multisigPda);
  const idx = BigInt(ms.transactionIndex.toString()) + 1n;
  const msg = new TransactionMessage({
    payerKey: vault,
    recentBlockhash: (await c.getLatestBlockhash()).blockhash,
    instructions: [SystemProgram.transfer({ fromPubkey: vault, toPubkey: vault, lamports: 0 })],
  });
  const ok = (s: string) => c.confirmTransaction(s, "confirmed");
  await ok(await m.rpc.vaultTransactionCreate({ connection: c, feePayer: payer, multisigPda, transactionIndex: idx,
    creator: m1.publicKey, rentPayer: payer.publicKey, vaultIndex: 0, ephemeralSigners: 0, transactionMessage: msg, signers: [payer, m1] }));
  await sendAndConfirmTransaction(c, new Transaction().add(
    m.instructions.proposalCreate({ multisigPda, transactionIndex: idx, creator: m1.publicKey, rentPayer: payer.publicKey }),
  ), [payer, m1], { commitment: "confirmed" });
  await ok(await m.rpc.proposalApprove({ connection: c, feePayer: payer, member: m1, multisigPda, transactionIndex: idx }));
  try {
    await m.rpc.vaultTransactionExecute({ connection: c, feePayer: payer, multisigPda, transactionIndex: idx,
      member: m1.publicKey, signers: [payer, m1] });
  } catch {
    console.log("single-approval execute rejected ✓");
    return;
  }
  throw new Error("vault transaction executed with ONE approval");
}

/** Token ACL CreateConfig (0): the mint's freeze authority → its MintConfig
 * PDA, as ops/gate `wire.ts acl` does (gating program: any id will do here). */
async function aclConfig(): Promise<void> {
  const [, mintArg, authPath] = process.argv.slice(2);
  const c = new Connection(process.env.RPC_URL!, "confirmed");
  const auth = kp(authPath!);
  const mint = new PublicKey(mintArg!);
  const acl = new PublicKey("TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP");
  const [mc] = PublicKey.findProgramAddressSync([Buffer.from("MINT_CONFIG"), mint.toBuffer()], acl);
  const k = (pubkey: PublicKey, isWritable: boolean, isSigner: boolean) => ({ pubkey, isWritable, isSigner });
  await sendAndConfirmTransaction(c, new Transaction().add(new TransactionInstruction({
    programId: acl,
    keys: [k(auth.publicKey, true, true), k(auth.publicKey, false, true), k(mint, true, false), k(mc, true, false),
      k(SystemProgram.programId, false, false), k(new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"), false, false)],
    data: Buffer.concat([Buffer.from([0]), SystemProgram.programId.toBuffer()]),
  })), [auth], { commitment: "confirmed" });
  console.log(mc.toBase58());
}

/** npcs-treasury `deposit(amount)` (#148): USDC in, the same value of the asset minted out. */
async function treasuryDeposit(): Promise<void> {
  const [, programArg, mintArg, usdcArg, depositorPath, amountArg] = process.argv.slice(2);
  const c = new Connection(process.env.RPC_URL!, "confirmed");
  const program = new PublicKey(programArg!);
  const mint = new PublicKey(mintArg!);
  const usdc = new PublicKey(usdcArg!);
  const d = kp(depositorPath!);
  const t2022 = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
  const tk = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
  const ata = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
  const atA = (owner: PublicKey, m: PublicKey, tp: PublicKey) => PublicKey.findProgramAddressSync([owner.toBuffer(), tp.toBuffer(), m.toBuffer()], ata)[0];
  const [treasury] = PublicKey.findProgramAddressSync([Buffer.from("treasury"), mint.toBuffer()], program);
  const [signer] = PublicKey.findProgramAddressSync([Buffer.from("treasury_signer"), treasury.toBuffer()], program);
  const [vault] = PublicKey.findProgramAddressSync([Buffer.from("vault"), treasury.toBuffer()], program);
  const disc = createHash("sha256").update("global:deposit").digest().subarray(0, 8);
  const amt = Buffer.alloc(8);
  amt.writeBigUInt64LE(BigInt(amountArg!));
  const k = (pubkey: PublicKey, isWritable: boolean, isSigner = false) => ({ pubkey, isWritable, isSigner });
  const sig = await sendAndConfirmTransaction(c, new Transaction().add(new TransactionInstruction({
    programId: program,
    keys: [k(treasury, true), k(vault, true), k(signer, false), k(mint, true), k(usdc, false), k(d.publicKey, false, true),
      k(atA(d.publicKey, usdc, tk), true), k(atA(d.publicKey, mint, t2022), true), k(tk, false), k(t2022, false)],
    data: Buffer.concat([disc, amt]),
  })), [d], { commitment: "confirmed" });
  console.log(sig);
}

const [cmd] = process.argv.slice(2);
const run: Record<string, () => Promise<void>> = {
  "config-pda": async () => console.log(m.getProgramConfigPda({})[0].toBase58()),
  treasury: async () => {
    const c = new Connection("https://api.devnet.solana.com", "confirmed");
    console.log((await m.accounts.ProgramConfig.fromAccountAddress(c, m.getProgramConfigPda({})[0])).treasury.toBase58());
  },
  "single-approval": singleApprovalIsNotEnough,
  "acl-config": aclConfig,
  "treasury-deposit": treasuryDeposit,
};
run[cmd!]!().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
