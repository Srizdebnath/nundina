import { describe, expect, it } from "vitest";
import { band, disclose } from "@/lib/prints";

describe("print disclosure (ME-P6)", () => {
  it("bands like mark-engine's band() (its unit-test vectors)", () => {
    expect(band(1_234_567n, 100n)).toBe((1_234_567n / 12_345n) * 12_345n);
    expect(band(5n, 100n)).toBe(5n); // width floors at one micro
  });

  it("discloses per class, as the PrintRecorded event does", () => {
    expect(disclose("Public", 945_000n, 2_000_000n, 100n)).toEqual({ price: 945_000n, volume: 2_000_000n });
    expect(disclose("Banded", 945_123n, 2_000_000n, 100n)).toEqual({ price: (945_123n / 9_451n) * 9_451n, volume: 2_000_000n });
    expect(disclose("AggregateOnly", 945_000n, 2_000_000n, 100n)).toEqual({ price: null, volume: 2_000_000n });
    expect(disclose("DataRoom", 945_000n, 2_000_000n, 100n)).toEqual({ price: null, volume: null });
  });
});
