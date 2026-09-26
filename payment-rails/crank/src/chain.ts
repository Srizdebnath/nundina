/** Read-side helpers: decoded program accounts, clock, token accounts. */
import {
  address,
  getBase58Decoder,
  getBase64Encoder,
  type Address,
  type Base58EncodedBytes,
  type Rpc,
  type SolanaRpcApi,
} from "@solana/kit";
import { accountDiscriminator, decodeAccount, PROGRAM_ID, type Struct } from "./idl.ts";

const b64 = getBase64Encoder();
const b58 = getBase58Decoder();
const CLOCK = address("SysvarC1ock11111111111111111111111111111111");

export type R = Rpc<SolanaRpcApi>;

export async function rawAccount(rpc: R, a: Address): Promise<{ owner: Address; data: Uint8Array } | null> {
  const { value } = await rpc.getAccountInfo(a, { encoding: "base64", commitment: "confirmed" }).send();
  if (!value) return null;
  return { owner: value.owner, data: new Uint8Array(b64.encode(value.data[0])) };
}

export async function fetchDecoded<T = Struct>(rpc: R, name: string, a: Address): Promise<T | null> {
  const raw = await rawAccount(rpc, a);
  if (!raw || raw.owner !== PROGRAM_ID) return null;
  return decodeAccount<T>(name, raw.data);
}

export async function exists(rpc: R, a: Address): Promise<boolean> {
  return (await rawAccount(rpc, a)) !== null;
}

/** Cluster `unix_timestamp` from the Clock sysvar (what the program sees). */
export async function clockNow(rpc: R): Promise<bigint> {
  const raw = await rawAccount(rpc, CLOCK);
  if (!raw) throw new Error("clock sysvar unavailable");
  return new DataView(raw.data.buffer, raw.data.byteOffset).getBigInt64(32, true);
}

/** All program accounts of type `name` whose `offset..offset+32` equals `key`. */
export async function listByKey<T = Struct>(
  rpc: R,
  name: string,
  offset: number,
  key: Address,
): Promise<{ address: Address; data: T }[]> {
  const disc = b58.decode(accountDiscriminator(name)) as Base58EncodedBytes;
  const res = await rpc
    .getProgramAccounts(PROGRAM_ID, {
      encoding: "base64",
      commitment: "confirmed",
      filters: [
        { memcmp: { offset: 0n, bytes: disc, encoding: "base58" } },
        { memcmp: { offset: BigInt(offset), bytes: key as unknown as Base58EncodedBytes, encoding: "base58" } },
      ],
    })
    .send();
  const out: { address: Address; data: T }[] = [];
  for (const r of res) {
    const data = decodeAccount<T>(name, new Uint8Array(b64.encode(r.account.data[0])));
    if (data) out.push({ address: r.pubkey, data });
  }
  return out.sort((a, b) => (a.address < b.address ? -1 : 1));
}

/** Token balance in base units; `null` if the account doesn't exist. */
export async function tokenBalance(rpc: R, a: Address): Promise<bigint | null> {
  try {
    const { value } = await rpc.getTokenAccountBalance(a, { commitment: "confirmed" }).send();
    return BigInt(value.amount);
  } catch {
    return null;
  }
}

/**
 * A token account `owner` holds for `mint` (deterministic: lowest address).
 * Refunds and fills only pay accounts the owner controls; the program
 * re-checks ownership, so picking any of them is safe.
 */
export async function ownerTokenAccount(rpc: R, owner: Address, mint: Address): Promise<Address | null> {
  const { value } = await rpc
    .getTokenAccountsByOwner(owner, { mint }, { encoding: "base64", commitment: "confirmed" })
    .send();
  if (value.length === 0) return null;
  return value.map((v) => v.pubkey).sort()[0]!;
}

/** Token program that owns `mint` (SPL Token or Token-2022). */
export async function tokenProgramOf(rpc: R, mint: Address): Promise<Address> {
  const raw = await rawAccount(rpc, mint);
  if (!raw) throw new Error(`mint ${mint} not found`);
  return raw.owner;
}
