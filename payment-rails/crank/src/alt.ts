/**
 * Address lookup tables for the crank's widest transactions.
 *
 * An auction-engine `settle` forwards the whole rails `settle_fill` account
 * set plus the gate's `settle_check` accounts — ~30 accounts, 1,684 bytes as
 * a plain v0 transaction, over the 1,232-byte packet limit. LiteSVM has no
 * packet limit, so this only showed on a real validator (#144 localnet e2e).
 * The crank therefore puts the non-signer accounts in a lookup table it owns
 * and sends the step as a compressed v0 message.
 *
 * Tables are created by whoever runs the crank (permissionless, rent only)
 * and extended when a later fill brings new accounts.
 */
import {
  AccountRole,
  address,
  getAddressEncoder,
  getProgramDerivedAddress,
  type Address,
  type AddressesByLookupTableAddress,
  type Instruction,
} from "@solana/kit";
import { send, type Ctx } from "./send.ts";

export const ALT_PROGRAM = address("AddressLookupTab1e1111111111111111111111111");
const SYSTEM = address("11111111111111111111111111111111");
/** Addresses per extend: keeps each extend transaction under the packet limit. */
export const EXTEND_CHUNK = 20;

const enc = getAddressEncoder();
const u32 = (n: number) => {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n, true);
  return b;
};
const u64 = (n: bigint) => {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigUint64(0, n, true);
  return b;
};
const cat = (...p: Uint8Array[]) => {
  const out = new Uint8Array(p.reduce((s, x) => s + x.length, 0));
  let o = 0;
  for (const x of p) {
    out.set(x, o);
    o += x.length;
  }
  return out;
};

export async function lookupTablePda(authority: Address, recentSlot: bigint): Promise<[Address, number]> {
  const [a, bump] = await getProgramDerivedAddress({
    programAddress: ALT_PROGRAM,
    seeds: [enc.encode(authority), u64(recentSlot)],
  });
  return [a, bump];
}

/** `CreateLookupTable { recent_slot, bump_seed }` (instruction 0). */
export function createLookupTableIx(table: Address, authority: Address, payer: Address, recentSlot: bigint, bump: number): Instruction {
  return {
    programAddress: ALT_PROGRAM,
    accounts: [
      { address: table, role: AccountRole.WRITABLE },
      { address: authority, role: AccountRole.READONLY_SIGNER },
      { address: payer, role: AccountRole.WRITABLE_SIGNER },
      { address: SYSTEM, role: AccountRole.READONLY },
    ],
    data: cat(u32(0), u64(recentSlot), Uint8Array.of(bump)),
  };
}

/** `ExtendLookupTable { new_addresses }` (instruction 2). */
export function extendLookupTableIx(table: Address, authority: Address, payer: Address, addresses: Address[]): Instruction {
  return {
    programAddress: ALT_PROGRAM,
    accounts: [
      { address: table, role: AccountRole.WRITABLE },
      { address: authority, role: AccountRole.READONLY_SIGNER },
      { address: payer, role: AccountRole.WRITABLE_SIGNER },
      { address: SYSTEM, role: AccountRole.READONLY },
    ],
    data: cat(u32(2), u64(BigInt(addresses.length)), ...addresses.map((a) => enc.encode(a) as Uint8Array)),
  };
}

/** Accounts worth putting in a table: everything that does not sign. */
export function tableCandidates(ixs: Instruction[]): Address[] {
  const seen = new Set<Address>();
  for (const ix of ixs) {
    for (const m of ix.accounts ?? []) {
      if (m.role === AccountRole.READONLY_SIGNER || m.role === AccountRole.WRITABLE_SIGNER) continue;
      seen.add(m.address);
    }
  }
  // An address that signs anywhere must stay static.
  for (const ix of ixs) for (const m of ix.accounts ?? []) if (m.role >= AccountRole.READONLY_SIGNER) seen.delete(m.address);
  return [...seen];
}

/** One lookup table the crank owns, grown on demand. */
export class CrankLookupTable {
  table: Address | null = null;
  addresses: Address[] = [];

  /** Make sure every non-signer account of `ixs` is in the table; returns it for `send`. */
  async cover(ctx: Ctx, ixs: Instruction[]): Promise<AddressesByLookupTableAddress> {
    const missing = tableCandidates(ixs).filter((a) => !this.addresses.includes(a));
    if (missing.length > 0) {
      const payer = ctx.payer.address;
      if (!this.table) {
        const slot = await ctx.rpc.getSlot({ commitment: "finalized" }).send();
        const [table, bump] = await lookupTablePda(payer, slot);
        const first = missing.splice(0, EXTEND_CHUNK);
        const r = await send(ctx, `lookup table create (${first.length})`, [
          createLookupTableIx(table, payer, payer, slot, bump),
          extendLookupTableIx(table, payer, payer, first),
        ]);
        if (r.status !== "sent") throw new Error(`lookup table create failed: ${r.name ?? r.message}`);
        this.table = table;
        this.addresses.push(...first);
      }
      while (missing.length > 0) {
        const chunk = missing.splice(0, EXTEND_CHUNK);
        const r = await send(ctx, `lookup table extend (${chunk.length})`, [extendLookupTableIx(this.table, payer, payer, chunk)]);
        if (r.status !== "sent") throw new Error(`lookup table extend failed: ${r.name ?? r.message}`);
        this.addresses.push(...chunk);
      }
      // New entries are usable from the next slot on.
      const at = await ctx.rpc.getSlot({ commitment: "confirmed" }).send();
      for (let i = 0; i < 40; i++) {
        if ((await ctx.rpc.getSlot({ commitment: "confirmed" }).send()) > at) break;
        await new Promise((r) => setTimeout(r, 250));
      }
    }
    return { [this.table!]: this.addresses };
  }
}
