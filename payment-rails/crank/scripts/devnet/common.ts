/** Shared plumbing for the R5 devnet scripts (setup, demo). */
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import {
  address,
  createAddressWithSeed,
  type Address,
  type Instruction,
  type KeyPairSigner,
} from "@solana/kit";
import { getCreateAccountWithSeedInstruction, getTransferSolInstruction } from "@solana-program/system";
import {
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstruction,
  getInitializeAccount3Instruction,
  getTokenSize,
  getTransferCheckedInstruction,
} from "@solana-program/token";
import { clockNow, exists } from "../../src/chain.ts";
import { send, type Ctx } from "../../src/send.ts";

/** Circle's devnet USDC. The rails config pins its settlement mint forever. */
export const CIRCLE_DEVNET_USDC = address("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU");

export function clusterLabel(rpcUrl: string): string {
  if (rpcUrl.includes("devnet")) return "devnet";
  if (rpcUrl.includes("mainnet")) return "mainnet";
  return "localnet";
}

export function explorer(sig: string, rpcUrl: string): string {
  const label = clusterLabel(rpcUrl);
  const q = label === "localnet" ? `cluster=custom&customUrl=${encodeURIComponent(rpcUrl)}` : `cluster=${label}`;
  return `https://explorer.solana.com/tx/${sig}?${q}`;
}

/** `.devnet/<cluster>/` — gitignored; holds addresses and demo keypairs. */
export function stateDir(rpcUrl: string): string {
  const dir = join(new URL("../../.devnet", import.meta.url).pathname, clusterLabel(rpcUrl));
  mkdirSync(dir, { recursive: true });
  return dir;
}

export interface Deployment {
  cluster: string;
  rpcUrl: string;
  programId: string;
  authority: string;
  config: string;
  usdcMint: string;
  usdcTokenProgram: string;
  feeReceiver: string;
  reserve: string;
  feeBps: number;
  reserveBps: number;
}

export function readDeployment(rpcUrl: string): Deployment {
  const f = join(stateDir(rpcUrl), "deployment.json");
  if (!existsSync(f)) throw new Error(`no ${f} — run setup.ts first`);
  return JSON.parse(readFileSync(f, "utf8"));
}

export function writeJson(rpcUrl: string, name: string, v: unknown): string {
  const f = join(stateDir(rpcUrl), name);
  writeFileSync(f, JSON.stringify(v, (_, x) => (typeof x === "bigint" ? x.toString() : x), 2) + "\n");
  return f;
}

export async function must(ctx: Ctx, label: string, ixs: Instruction[], cuLimit?: number): Promise<string> {
  const r = await send(ctx, label, ixs, { cuLimit });
  if (r.status !== "sent") throw new Error(`${label}: ${r.name ?? r.message}`);
  return r.signature;
}

export async function waitUntil(ctx: Ctx, t: bigint, label: string): Promise<void> {
  let last = -1n;
  for (;;) {
    const now = await clockNow(ctx.rpc);
    if (now >= t) return;
    if (last === -1n || now - last >= 10n) {
      ctx.log(`… waiting for ${label}: ${t - now}s`);
      last = now;
    }
    await new Promise((r) => setTimeout(r, 1_000));
  }
}

/** Associated token account of `owner` for `mint`, created if missing. */
export async function ensureAta(ctx: Ctx, owner: Address, mint: Address, tokenProgram: Address): Promise<Address> {
  const [ata] = await findAssociatedTokenPda({ owner, mint, tokenProgram });
  if (!(await exists(ctx.rpc, ata))) {
    await must(ctx, `create ATA ${ata.slice(0, 8)}…`, [
      getCreateAssociatedTokenIdempotentInstruction({ payer: ctx.payer, owner, mint, ata, tokenProgram }),
    ]);
  }
  return ata;
}

/**
 * A second, deterministic token account of `base` for `mint`
 * (`createAccountWithSeed`) — e.g. the reserve, which must be distinct from
 * the fee receiver without needing another keypair to safeguard.
 */
export async function ensureSeedTokenAccount(
  ctx: Ctx,
  base: KeyPairSigner,
  seed: string,
  mint: Address,
  tokenProgram: Address,
): Promise<Address> {
  const acct = await createAddressWithSeed({ baseAddress: base.address, programAddress: tokenProgram, seed });
  if (!(await exists(ctx.rpc, acct))) {
    const space = getTokenSize();
    const amount = await ctx.rpc.getMinimumBalanceForRentExemption(BigInt(space)).send();
    await must(ctx, `create ${seed} account`, [
      getCreateAccountWithSeedInstruction({
        payer: ctx.payer, newAccount: acct, baseAccount: base, base: base.address, seed,
        amount, space, programAddress: tokenProgram,
      }),
      getInitializeAccount3Instruction({ account: acct, mint, owner: base.address }, { programAddress: tokenProgram }),
    ]);
  }
  return acct;
}

export function transferSol(from: KeyPairSigner, to: Address, lamports: bigint): Instruction {
  return getTransferSolInstruction({ source: from, destination: to, amount: lamports });
}

export function transferToken(
  from: Address, to: Address, mint: Address, owner: KeyPairSigner, amount: bigint, tokenProgram: Address,
): Instruction {
  return getTransferCheckedInstruction({ source: from, destination: to, mint, authority: owner, amount, decimals: 6 }, { programAddress: tokenProgram });
}
