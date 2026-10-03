/** Helpers for test-localnet.sh only (not part of the migration tool). */
import * as m from "@sqds/multisig";
import { Connection, Keypair, PublicKey, SystemProgram, Transaction, TransactionMessage, sendAndConfirmTransaction } from "@solana/web3.js";
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

const [cmd] = process.argv.slice(2);
const run: Record<string, () => Promise<void>> = {
  "config-pda": async () => console.log(m.getProgramConfigPda({})[0].toBase58()),
  treasury: async () => {
    const c = new Connection("https://api.devnet.solana.com", "confirmed");
    console.log((await m.accounts.ProgramConfig.fromAccountAddress(c, m.getProgramConfigPda({})[0])).treasury.toBase58());
  },
  "single-approval": singleApprovalIsNotEnough,
};
run[cmd!]!().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
