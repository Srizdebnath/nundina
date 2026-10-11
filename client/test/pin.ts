/**
 * Shared IDL conformance harness for the transaction builders.
 *
 * A builder is "pinned" when its account list, writability flags, signer flags
 * and instruction data match what the program's IDL declares — so a change in
 * account order, a dropped `has_one`, or a silently de-signed account fails a
 * test instead of failing on chain.
 *
 * `deSigned` covers the CPI case: when the engine forwards an account it signs
 * for itself over CPI, the client passes it un-signed and the engine supplies
 * the signature.
 */
import { expect } from "vitest";
import { AccountRole, address, getAddressEncoder, getProgramDerivedAddress, type Address, type Instruction } from "@solana/kit";

export interface IdlAcc {
  name: string;
  writable?: boolean;
  signer?: boolean;
  address?: string;
  pda?: { seeds: { kind: string; value?: number[]; path?: string }[] };
  accounts?: IdlAcc[];
}
export type IdlLike = {
  address: string;
  instructions: { name: string; discriminator: number[]; accounts: IdlAcc[]; args: { name: string; type: unknown }[] }[];
};

export const flat = (as: IdlAcc[]): IdlAcc[] => as.flatMap((a) => (a.accounts ? flat(a.accounts) : [a]));
export const idlIx = (idl: IdlLike, name: string) => idl.instructions.find((i) => i.name === name)!;

export const writable = (r: AccountRole) => r === AccountRole.WRITABLE || r === AccountRole.WRITABLE_SIGNER;
export const signer = (r: AccountRole) => r === AccountRole.READONLY_SIGNER || r === AccountRole.WRITABLE_SIGNER;

/**
 * `ix.accounts[offset..]` against instruction `name` of `idl`: same count
 * (unless more follow), same addresses (`expected`, else the IDL's fixed
 * `address`), same writability, same signer flag except for `deSigned`.
 */
export function pin(
  ix: Instruction,
  idl: IdlLike,
  name: string,
  expected: Record<string, Address>,
  offset = 0,
  deSigned: string[] = [],
) {
  const metas = flat(idlIx(idl, name).accounts);
  const accs = ix.accounts!.slice(offset, offset + metas.length);
  expect(accs, name).toHaveLength(metas.length);
  metas.forEach((m, i) => {
    const want = expected[m.name] ?? m.address;
    expect(want, `${name}.${m.name}: no expectation`).toBeDefined();
    expect(accs[i]!.address, `${name}.${m.name}`).toBe(want);
    expect(writable(accs[i]!.role), `${name}.${m.name} writable`).toBe(!!m.writable);
    expect(signer(accs[i]!.role), `${name}.${m.name} signer`).toBe(!!m.signer && !deSigned.includes(m.name));
  });
  return metas.length;
}

/** Data = discriminator ‖ args (u64 / i64 little-endian, in IDL order). */
export function data(idl: IdlLike, name: string, args: Record<string, bigint>): number[] {
  const i = idlIx(idl, name);
  const out = [...i.discriminator];
  for (const a of i.args) {
    const b = new DataView(new ArrayBuffer(8));
    if (a.type === "u64") b.setBigUint64(0, args[a.name]!, true);
    else if (a.type === "i64") b.setBigInt64(0, args[a.name]!, true);
    else throw new Error(`test encoder: ${String(a.type)}`);
    out.push(...new Uint8Array(b.buffer));
  }
  return out;
}

/** A PDA from the IDL's own `const` seeds (the program's seed bytes, not ours). */
export async function idlPda(idl: IdlLike, ixName: string, acc: string, dynamic: Uint8Array[]): Promise<Address> {
  const m = flat(idlIx(idl, ixName).accounts).find((a) => a.name === acc)!;
  let d = 0;
  const seeds = m.pda!.seeds.map((s) => (s.kind === "const" ? Uint8Array.from(s.value!) : dynamic[d++]!));
  const [a] = await getProgramDerivedAddress({ programAddress: address(idl.address), seeds });
  return a;
}

export const enc = getAddressEncoder();
export const A = (a: Address) => enc.encode(a) as Uint8Array;
export const le = (n: bigint) => {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigUint64(0, n, true);
  return b;
};

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
/** Minimal base58 for fixture addresses; the SDK only accepts these back. */
function b58(bytes: Uint8Array): string {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let out = "";
  while (n > 0n) {
    out = B58[Number(n % 58n)] + out;
    n /= 58n;
  }
  // one leading '1' per LEADING zero byte
  let lead = 0;
  while (lead < bytes.length && bytes[lead] === 0) lead++;
  return "1".repeat(lead) + out;
}

/**
 * `k(n)` — a stable, distinct, valid base58 address for index `n`.
 * Byte `0` is forced non-zero so the leading-`1` padding stays empty and the
 * encoding is always 43–44 characters, inside Solana's accepted range.
 */
export const k = (n: number): Address => {
  const bytes = new Uint8Array(32);
  bytes[0] = (n % 251) + 1;
  for (let i = 1; i < 32; i++) bytes[i] = (n * 31 + i * 7) & 0xff;
  return address(b58(bytes));
};
