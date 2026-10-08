import { describe, expect, it } from "vitest";
import { address } from "@solana/kit";
import { mintSupply, tranchePosition } from "@/lib/tranche";
import { TOKEN_PROGRAM } from "@/lib/programs";

const pool = address("CvcaeyuPu7SrFJ91WniJfBhUbQBkb8QQVL9fyK4wcFAS");
const senior = address("SysvarC1ock11111111111111111111111111111111");
const junior = address("SysvarRent111111111111111111111111111111111");
const p = {
  pool_id: 7,
  state: "Impaired",
  senior_mint: senior,
  junior_mint: junior,
  senior_principal: 800_000_000n,
  junior_principal: 200_000_000n,
  senior_loss: 0n,
  junior_loss: 50_000_000n,
  params: { senior_coupon_bps: 600, junior_coupon_bps: 1_400 },
};
const h = (mint: typeof senior, amount: bigint, frozen = false) => ({
  account: address("11111111111111111111111111111112"),
  mint,
  amount,
  decimals: 6,
  frozen,
  program: TOKEN_PROGRAM,
});

describe("tranche positions", () => {
  it("prorates principal and the tranche's realised loss by share of supply", () => {
    const j = tranchePosition(pool, p, h(junior, 50_000_000n, true), 200_000_000n)!;
    expect(j).toMatchObject({ side: "junior", shareBps: 2_500n, principal: 50_000_000n, loss: 12_500_000n, locked: true, couponBps: 1_400, state: "Impaired" });
    const s = tranchePosition(pool, p, h(senior, 800_000_000n), 800_000_000n)!;
    expect(s).toMatchObject({ side: "senior", shareBps: 10_000n, principal: 800_000_000n, loss: 0n, locked: false });
  });

  it("ignores other mints, empty holdings, and survives a zero supply", () => {
    expect(tranchePosition(pool, p, h(address("SysvarS1otHashes111111111111111111111111111"), 1n), 1n)).toBeNull();
    expect(tranchePosition(pool, p, h(senior, 0n), 1n)).toBeNull();
    expect(tranchePosition(pool, p, h(senior, 5n), 0n)!.principal).toBe(0n);
  });

  it("reads a mint's supply at byte 36", () => {
    const d = new Uint8Array(82);
    new DataView(d.buffer).setBigUint64(36, 123_456_789n, true);
    expect(mintSupply(d)).toBe(123_456_789n);
    expect(() => mintSupply(new Uint8Array(40))).toThrow();
  });
});
