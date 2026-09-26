/** Program-derived addresses — the same seeds as `state.rs`. */
import { getAddressEncoder, getProgramDerivedAddress, type Address } from "@solana/kit";
import { PROGRAM_ID } from "./idl.ts";

const utf8 = new TextEncoder();
const addr = getAddressEncoder();

export function le(n: bigint | number, bytes: number): Uint8Array {
  const out = new Uint8Array(bytes);
  let v = BigInt(n);
  for (let i = 0; i < bytes; i++) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

/** `[literal, ...addresses-or-bytes]` → PDA of the rails program. */
async function pda(literal: string, ...rest: (Address | Uint8Array)[]): Promise<Address> {
  const seeds = [
    utf8.encode(literal),
    ...rest.map((s) => (typeof s === "string" ? addr.encode(s) : s)),
  ];
  const [a] = await getProgramDerivedAddress({ programAddress: PROGRAM_ID, seeds });
  return a;
}

export const configPda = () => pda("rails_config");
export const sessionPda = (operator: Address, id: bigint) => pda("session", operator, le(id, 8));
export const escrowPda = (session: Address, owner: Address) => pda("escrow", session, owner);
export const vaultPda = (escrow: Address) => pda("vault", escrow);
export const assetVaultPda = (escrow: Address) => pda("asset_vault", escrow);
export const receiptPda = (session: Address, fillIndex: number) => pda("fill", session, le(fillIndex, 4));
export const destinationPda = (owner: Address) => pda("payout", owner);
export const facilityPda = (curator: Address, id: bigint) => pda("facility", curator, le(id, 8));
export const facilityVaultPda = (facility: Address) => pda("facility_vault", facility);
export const advancePda = (facility: Address, borrower: Address) => pda("advance", facility, borrower);
export const couponPda = (distributor: Address, period: bigint) => pda("coupon", distributor, le(period, 8));
