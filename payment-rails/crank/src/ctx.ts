/** Build a crank context from env: RPC_URL, WS_URL, KEYPAIR. */
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import {
  createKeyPairSignerFromBytes,
  createSolanaRpc,
  createSolanaRpcSubscriptions,
  type KeyPairSigner,
} from "@solana/kit";
import { DEFAULT_FEE, type Ctx } from "./send.ts";

export async function loadKeypair(path: string): Promise<KeyPairSigner> {
  const p = path.replace(/^~(?=\/)/, homedir());
  return createKeyPairSignerFromBytes(Uint8Array.from(JSON.parse(readFileSync(p, "utf8"))));
}

/** ws URL for an http RPC URL: same host, scheme ws(s), port + 1 if explicit. */
export function wsFor(rpcUrl: string): string {
  const u = new URL(rpcUrl);
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  if (u.port) u.port = String(Number(u.port) + 1);
  return u.toString();
}

export async function makeCtx(opts: {
  rpcUrl?: string;
  wsUrl?: string;
  payer?: KeyPairSigner;
  keypairPath?: string;
  quiet?: boolean;
} = {}): Promise<Ctx> {
  const rpcUrl = opts.rpcUrl ?? process.env.RPC_URL ?? "http://127.0.0.1:8899";
  const wsUrl = opts.wsUrl ?? process.env.WS_URL ?? wsFor(rpcUrl);
  const payer =
    opts.payer ?? (await loadKeypair(opts.keypairPath ?? process.env.KEYPAIR ?? "~/.config/solana/id.json"));
  return {
    rpc: createSolanaRpc(rpcUrl),
    rpcSubscriptions: createSolanaRpcSubscriptions(wsUrl),
    payer,
    fee: {
      ...DEFAULT_FEE,
      minMicroLamports: BigInt(process.env.PRIORITY_FEE_MIN ?? DEFAULT_FEE.minMicroLamports),
      maxMicroLamports: BigInt(process.env.PRIORITY_FEE_MAX ?? DEFAULT_FEE.maxMicroLamports),
    },
    maxAttempts: Number(process.env.MAX_ATTEMPTS ?? 5),
    baseDelayMs: Number(process.env.RETRY_BASE_MS ?? 500),
    log: opts.quiet ? () => {} : (m) => console.log(m),
  };
}
