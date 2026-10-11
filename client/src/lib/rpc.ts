/**
 * Chain reads. Everything the UI shows is read from the cluster at request
 * time (server components) or polled (client components); there is no
 * cache and no backend of our own between the page and the programs.
 */
import {
  createDefaultRpcTransport,
  createSolanaRpcFromTransport,
  address,
  type Address,
  type Base64EncodedBytes,
  type RpcTransport,
} from "@solana/kit";
import { fromBase64 } from "./borsh";
import { RPC_URL } from "./programs";

const base = createDefaultRpcTransport({ url: RPC_URL });

/** Public devnet RPC drops connections under load: retry network errors (not RPC errors). */
const retrying: RpcTransport = async <T>(req: Parameters<RpcTransport>[0]): Promise<T> => {
  for (let attempt = 0; ; attempt++) {
    try {
      return await base<T>(req);
    } catch (e) {
      const network = e instanceof TypeError || /fetch failed|ECONNRESET|ETIMEDOUT|socket/i.test(String((e as Error)?.message));
      if (!network || attempt >= 3) throw e;
      await new Promise((r) => setTimeout(r, 400 * 2 ** attempt));
    }
  }
};

export const rpc = createSolanaRpcFromTransport(retrying);

export interface RawAccount {
  address: Address;
  owner: Address;
  lamports: bigint;
  data: Uint8Array;
}

export async function getAccount(a: Address): Promise<RawAccount | null> {
  const { value } = await rpc.getAccountInfo(a, { encoding: "base64", commitment: "confirmed" }).send();
  if (!value) return null;
  return { address: a, owner: value.owner, lamports: BigInt(value.lamports), data: fromBase64(value.data[0]) };
}

export async function getAccounts(as: Address[]): Promise<(RawAccount | null)[]> {
  const out: (RawAccount | null)[] = [];
  for (let i = 0; i < as.length; i += 100) {
    const chunk = as.slice(i, i + 100);
    const { value } = await rpc.getMultipleAccounts(chunk, { encoding: "base64", commitment: "confirmed" }).send();
    value.forEach((v, j) =>
      out.push(v ? { address: chunk[j]!, owner: v.owner, lamports: BigInt(v.lamports), data: fromBase64(v.data[0]) } : null),
    );
  }
  return out;
}

/** Accounts of `program` whose first 8 bytes are `discriminator` (+ memcmps). */
export async function getProgramAccounts(
  program: Address,
  discriminator: Uint8Array,
  memcmp: { offset: number; bytes: Address }[] = [],
): Promise<RawAccount[]> {
  const b64 = btoa(String.fromCharCode(...discriminator)) as Base64EncodedBytes;
  const value = await rpc
    .getProgramAccounts(program, {
      encoding: "base64",
      commitment: "confirmed",
      filters: [
        { memcmp: { offset: 0n, bytes: b64, encoding: "base64" } },
        ...memcmp.map((m) => ({ memcmp: { offset: BigInt(m.offset), bytes: m.bytes as unknown as Base64EncodedBytes, encoding: "base58" as const } })),
      ],
    } as never)
    .send() as unknown as { pubkey: Address; account: { owner: Address; lamports: number | bigint; data: [string, string] } }[];
  return value.map((v) => ({
    address: v.pubkey,
    owner: v.account.owner,
    lamports: BigInt(v.account.lamports),
    data: fromBase64(v.account.data[0]),
  }));
}

export interface TokenHolding {
  account: Address;
  mint: Address;
  amount: bigint;
  decimals: number;
  frozen: boolean;
  program: Address;
}

/** Every SPL / Token-2022 account `owner` holds (parsed by the RPC). */
export async function getTokenHoldings(owner: Address): Promise<TokenHolding[]> {
  const out: TokenHolding[] = [];
  for (const programId of [
    address("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
    address("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
  ]) {
    const { value } = await rpc
      .getTokenAccountsByOwner(owner, { programId }, { encoding: "jsonParsed", commitment: "confirmed" })
      .send();
    for (const v of value) {
      const info = (v.account.data as unknown as { parsed: { info: Record<string, unknown> } }).parsed.info;
      const ta = info.tokenAmount as { amount: string; decimals: number };
      out.push({
        account: v.pubkey,
        mint: address(info.mint as string),
        amount: BigInt(ta.amount),
        decimals: ta.decimals,
        frozen: info.state === "frozen",
        program: programId,
      });
    }
  }
  return out;
}

/** The cluster's clock (block time of the latest confirmed slot). */
export async function chainNow(): Promise<number> {
  const slot = await rpc.getSlot({ commitment: "confirmed" }).send();
  const t = await rpc.getBlockTime(slot).send().catch(() => null);
  if (t === null) throw new Error(`no block time for slot ${slot}`);
  return Number(t);
}
