/**
 * Seasoning crank (SRS 08 §6 crank-runner, CG-P3, #127): find every
 * credit-gate lot ledger of a mint holding a lot whose seasoning has
 * expired, and send credit-gate's permissionless `merge_expired` for it.
 * Any wallet can run it; the program decides what merges.
 *
 * Reads credit-gate's `HolderLots` bytes. The layout is pinned on the Rust
 * side by `lots.rs` `layout_read_by_the_seasoning_crank`:
 * 582 bytes · mint @8 · lot_count @84 · lots @85, 31 bytes each,
 * restriction_expiry @+20 (i64 LE).
 */
import { createHash } from "node:crypto";
import { AccountRole, address, getAddressDecoder, getBase64Encoder, type Address, type Base58EncodedBytes } from "@solana/kit";
import { clockNow, type R } from "./chain.ts";
import type { Outcome } from "./crank.ts";
import { send, type Ctx } from "./send.ts";

export const CREDIT_GATE = address("5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX");
export const HOLDER_LOTS_LEN = 582;
const MINT_AT = 8;
const COUNT_AT = 84;
const LOTS_AT = 85;
const LOT_LEN = 31;
const EXPIRY_AT = 20;
const MAX_LOTS = 16;
const MERGE_EXPIRED = createHash("sha256").update("global:merge_expired").digest().subarray(0, 8);

/** Pure: does this ledger hold a lot that has seasoned by `now`? */
export function hasSeasonedLot(data: Uint8Array, now: bigint): boolean {
  if (data.length !== HOLDER_LOTS_LEN) return false;
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const count = Math.min(data[COUNT_AT]!, MAX_LOTS);
  for (let i = 0; i < count; i++) {
    if (view.getBigInt64(LOTS_AT + LOT_LEN * i + EXPIRY_AT, true) <= now) return true;
  }
  return false;
}

/** Ledgers of `mint` (credit-gate accounts of the HolderLots size with this mint). */
export async function listLedgers(rpc: R, mint: Address, program: Address = CREDIT_GATE) {
  const b64 = getBase64Encoder();
  const accounts = await rpc
    .getProgramAccounts(program, {
      encoding: "base64",
      commitment: "confirmed",
      filters: [
        { dataSize: BigInt(HOLDER_LOTS_LEN) },
        { memcmp: { offset: BigInt(MINT_AT), bytes: mint as string as Base58EncodedBytes, encoding: "base58" } },
      ],
    })
    .send();
  return accounts.map((a) => ({ address: a.pubkey, data: new Uint8Array(b64.encode(a.account.data[0])) }));
}

export async function seasonAll(ctx: Ctx, mint: Address, program: Address = CREDIT_GATE): Promise<Outcome[]> {
  const now = await clockNow(ctx.rpc);
  const out: Outcome[] = [];
  for (const { address: lots, data } of await listLedgers(ctx.rpc, mint, program)) {
    const holder = getAddressDecoder().decode(data.subarray(40, 72));
    const target = `lots ${holder}`;
    if (!hasSeasonedLot(data, now)) {
      out.push({ target, result: "not-yet", detail: "no lot past restriction_expiry" });
      continue;
    }
    const r = await send(ctx, `merge_expired ${holder.slice(0, 12)}…`, [{
      programAddress: program,
      accounts: [
        { address: lots, role: AccountRole.WRITABLE },
        { address: mint, role: AccountRole.READONLY },
      ],
      data: Uint8Array.from(MERGE_EXPIRED),
    }], { cuLimit: 60_000 });
    out.push(r.status === "sent"
      ? { target, result: "sent", signature: r.signature }
      : { target, result: "failed", detail: r.name ?? r.message });
  }
  return out;
}

