import { describe, expect, it } from "vitest";
import { address } from "@solana/kit";
import { fromHex, hex } from "@/lib/borsh";
import { clockPhase, curves, fmt6, gross, navToPrice, parseUnits, type OrderView } from "@/lib/session";
import { commitment } from "@/lib/tx";

const owner = address("BJL7L58MX82DiGxtjcq5VbzWuLLZe26U6R8PCfdJ2EMh");
const o = (side: "Buy" | "Sell", limit: bigint, qty: bigint, revealed = true, screened = false, eligible = false): OrderView => ({
  owner,
  side,
  commitment: new Uint8Array(32),
  escrowed: 0n,
  revealed,
  limit,
  qty,
  alloc: 0n,
  screened,
  eligible,
});

describe("session math", () => {
  it("builds the auction.rs book's step curves (bids best-first, asks cheapest-first)", () => {
    const { demand, supply } = curves([
      o("Sell", 900_000n, 10_000_000n),
      o("Buy", 950_000n, 6_000_000n),
      o("Buy", 970_000n, 6_000_000n),
      o("Buy", 930_000n, 4_000_000n),
      o("Buy", 999_000n, 1_000_000n, false), // sealed orders are not on the curve
    ]);
    expect(demand).toEqual([
      { price: 970_000n, qty: 6_000_000n },
      { price: 950_000n, qty: 12_000_000n },
      { price: 930_000n, qty: 16_000_000n },
    ]);
    expect(supply).toEqual([{ price: 900_000n, qty: 10_000_000n }]);
  });

  it("leaves orders the AE-P6 gate check excluded off the curves", () => {
    const { demand, supply } = curves([
      o("Sell", 900_000n, 10_000_000n, true, true, true),
      o("Buy", 990_000n, 10_000_000n, true, true, false), // excluded: not on the curve
      o("Buy", 950_000n, 6_000_000n, true, true, true),
      o("Buy", 930_000n, 6_000_000n, true, false), // not screened yet: still shown
    ]);
    expect(demand).toEqual([
      { price: 950_000n, qty: 6_000_000n },
      { price: 930_000n, qty: 12_000_000n },
    ]);
    expect(supply).toEqual([{ price: 900_000n, qty: 10_000_000n }]);
  });

  it("says which phase the clock is in, independent of crank lag", () => {
    const p = { commit_end: 100n, reveal_end: 200n, settle_deadline: 400n };
    expect(clockPhase(p, "Commit", 99)).toBe("Commit");
    expect(clockPhase(p, "Commit", 100)).toBe("Reveal");
    expect(clockPhase(p, "Reveal", 250)).toBe("Reveal");
    expect(clockPhase(p, "Settling", 400)).toBe("Expired");
    expect(clockPhase(p, "Printed", 999)).toBe("Printed");
  });

  it("formats fixed-point and NAV", () => {
    expect(fmt6(950_000n, 4)).toBe("0.9500");
    expect(fmt6(12_345_678_900n)).toBe("12,345.678900");
    expect(fmt6(-1n)).toBe("-0.000001");
    // NAV_ONE (1 USDC per whole unit) at 6 dp.
    expect(navToPrice(1_000_000_000_000n, 6)).toBe(1_000_000n);
  });

  it("parses decimal inputs exactly", () => {
    expect(parseUnits("0.95", 6)).toBe(950_000n);
    expect(parseUnits("12", 6)).toBe(12_000_000n);
    expect(() => parseUnits("0.0000001", 6)).toThrow(/at most 6/);
    expect(() => parseUnits("1e3", 6)).toThrow(/not a number/);
  });

  it("escrow gross rounds up like the program (auction-engine gross_rounds_up_like_the_rails)", () => {
    expect(gross(1_000_000n, 950_000n, 6)).toBe(950_000n);
    expect(gross(1n, 950_000n, 6)).toBe(1n);
    expect(gross(3n, 333_333n, 6)).toBe(1n);
    expect(gross(0n, 950_000n, 6)).toBe(0n);
    // 1.5 units at 0.333333: floor would escrow 499999 and reveal would fail.
    expect(gross(1_500_000n, 333_333n, 6)).toBe(500_000n);
  });
});

describe("commitment (AE-P3)", () => {
  it("hashes side ‖ limit ‖ qty ‖ nonce ‖ owner and binds every field", async () => {
    const nonce = fromHex("07".repeat(32));
    const c = await commitment("Buy", 950_000n, 5_000_000n, nonce, owner);
    expect(c).toHaveLength(32);
    const again = await commitment("Buy", 950_000n, 5_000_000n, nonce, owner);
    expect(hex(again)).toBe(hex(c));
    for (const other of [
      await commitment("Sell", 950_000n, 5_000_000n, nonce, owner),
      await commitment("Buy", 950_001n, 5_000_000n, nonce, owner),
      await commitment("Buy", 950_000n, 5_000_001n, nonce, owner),
      await commitment("Buy", 950_000n, 5_000_000n, fromHex("08".repeat(32)), owner),
      await commitment("Buy", 950_000n, 5_000_000n, nonce, address("7UdtTMgH4v8YSG8mWX2r8uASgJwAkxyo4h8yb5UEEbuh")),
    ]) {
      expect(hex(other)).not.toBe(hex(c));
    }
  });
});
