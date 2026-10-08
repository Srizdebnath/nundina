import assert from "node:assert/strict";
import { test } from "node:test";
import { AccountRole, address, generateKeyPairSigner } from "@solana/kit";
import { accountDiscriminator, decodeAccount, decodeAt, decodeEvent, encode, errorCode, errorName, idl, ix, loadIdl, makeCodec } from "../src/idl.ts";

const A = address("GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn");
const B = address("5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX");

test("settle_fill data = discriminator + u32 + u64 + u64 + [u8;32]", async () => {
  const op = await generateKeyPairSigner();
  type Def = { name: string; signer?: boolean; accounts?: Def[] };
  // Composite groups (the AE-P8 `gate` accounts, #133) nest.
  const fill = (defs: Def[]): Record<string, never> =>
    Object.fromEntries(defs.map((a) => [a.name, a.accounts ? fill(a.accounts) : a.signer ? op : A])) as never;
  const accounts = fill(idl.instructions.find((i) => i.name === "settle_fill")!.accounts);
  const i = ix("settle_fill", accounts, { fill_index: 7, qty: 1_500_000n, price: 98_123_456n, disclosure_hash: new Uint8Array(32).fill(9) });
  const d = i.data!;
  assert.equal(d.length, 8 + 4 + 8 + 8 + 32);
  const dv = new DataView(d.buffer, d.byteOffset);
  assert.equal(dv.getUint32(8, true), 7);
  assert.equal(dv.getBigUint64(12, true), 1_500_000n);
  assert.equal(dv.getBigUint64(20, true), 98_123_456n);
  assert.equal(d[28], 9);
  // Roles come from the IDL, not the caller.
  const role = (name: string) => i.accounts![idl.instructions.find((x) => x.name === "settle_fill")!.accounts.findIndex((a) => a.name === name)]!.role;
  assert.equal(role("operator"), AccountRole.WRITABLE_SIGNER);
  assert.equal(role("config"), AccountRole.READONLY);
  assert.equal(role("buyer_vault"), AccountRole.WRITABLE);
  // The gate group is flattened after the fill's own accounts, in IDL order:
  // 17 + 11, and the two lot ledgers are writable.
  assert.equal(i.accounts!.length, 28);
  assert.deepEqual(
    i.accounts!.slice(17).map((m) => m.role),
    [0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0].map((w) => (w ? AccountRole.WRITABLE : AccountRole.READONLY)),
  );
});

test("fixed-address accounts are filled in; signer slots demand signers", async () => {
  const op = await generateKeyPairSigner();
  const i = ix("finish_session", { session: A, operator: op });
  assert.equal(i.accounts!.length, 2);
  assert.throws(() => ix("finish_session", { session: A, operator: B }), /must be a signer/);
  assert.throws(() => ix("finish_session", { session: A }), /missing account operator/);
  assert.throws(() => ix("deposit", {} as never, {}), /missing account/);
});

test("Escrow round-trips and matches the on-chain size (8 + INIT_SPACE)", () => {
  const escrow = {
    session: A, owner: B, vault: A, side: "Sell", cash: 0n, asset: 2_000_000n,
    asset_vault: B, bond: 5_000_000n, revealed: true, bump: 254,
  };
  const body = encode({ defined: { name: "Escrow" } }, escrow);
  assert.equal(body.length, 155); // 3×32 + 1 + 8 + 8 + 32 + 8 + 1 + 1 (Escrow::INIT_SPACE)
  const data = new Uint8Array([...accountDiscriminator("Escrow"), ...body]);
  assert.deepEqual(decodeAccount("Escrow", data), escrow);
  // Wrong type → null, never a mis-decode.
  assert.equal(decodeAccount("CashSession", data), null);
});

test("integers: i64 sign, u64 bounds", () => {
  assert.equal(decodeAt("i64", encode("i64", -42n), 0)[0], -42n);
  assert.equal(decodeAt("u64", encode("u64", 2n ** 64n - 1n), 0)[0], 2n ** 64n - 1n);
  assert.throws(() => encode("u64", 2n ** 64n), /out of range/);
  assert.throws(() => encode("u64", -1n), /out of range/);
});

test("error codes map both ways (append-only codes)", () => {
  assert.equal(errorCode("Unauthorized"), 6000);
  assert.equal(errorName(6011), "DestinationNotAllowlisted");
  assert.equal(errorCode("DestinationRevoked"), 6034);
});

test("TransferInstruction (AE-P8) round-trips, including a Vec<LotRef>", () => {
  const disc = idl.events.find((e) => e.name === "TransferInstruction")!.discriminator;
  const ev = {
    session: A, fill_index: 3, from: B, to: A, qty: 2_000_000n, price: 1_000_411n,
    lot_ids: [
      { lot_index: 0, qty: 1_500_000n, acquired_ts: 1_790_000_000n, restriction_expiry: 1_790_604_800n },
      { lot_index: 0xffff_ffff, qty: 500_000n, acquired_ts: 0n, restriction_expiry: 0n },
    ],
    exemption_path: "Section4a7", gate_check: "Passed", disclosure_hash: new Uint8Array(32).fill(4),
  };
  const body = encode({ defined: { name: "TransferInstruction" } }, ev);
  // 32+4+32+32+8+8 | 4-byte vec len + 2×(4+8+8+8) | enum | enum | 32
  assert.equal(body.length, 116 + 4 + 56 + 1 + 1 + 32);
  assert.deepEqual(decodeEvent(new Uint8Array([...disc, ...body])), { name: "TransferInstruction", data: ev });
  // What fills settled before the gate check (#133) carry.
  const today = { ...ev, lot_ids: [], exemption_path: "NotRecorded", gate_check: "NotPerformed" };
  const b2 = encode({ defined: { name: "TransferInstruction" } }, today);
  assert.deepEqual(decodeEvent(new Uint8Array([...disc, ...b2]))!.data, today);
});

test("vec: truncated length prefix or body is rejected, never mis-decoded", () => {
  const t = { vec: "u64" } as const;
  const b = encode(t, [1n, 2n]);
  assert.deepEqual(decodeAt(t, b, 0)[0], [1n, 2n]);
  assert.throws(() => decodeAt(t, b.subarray(0, b.length - 1), 0), /too short/);
});

test("engine IDL types: u128 and option round-trip (mark-engine Feed has u128 NAV and option fields)", () => {
  const mark = makeCodec(loadIdl("mark_engine"));
  assert.equal(mark.decodeAt("u128", mark.encode("u128", 2n ** 128n - 1n), 0)[0], 2n ** 128n - 1n);
  assert.throws(() => mark.encode("u128", 2n ** 128n), /out of range/);
  assert.deepEqual(mark.decodeAt({ option: "u64" }, mark.encode({ option: "u64" }, null), 0), [null, 1]);
  assert.deepEqual(mark.decodeAt({ option: "u64" }, mark.encode({ option: "u64" }, 7n), 0), [7n, 9]);
  assert.throws(() => mark.decodeAt({ option: "u64" }, Uint8Array.of(2), 0), /bad option tag/);
  // FeedParams through the IDL, exactly as engines.ts sends it.
  const p = { warn_after: 1n, stale_after: 2n, warned_haircut_bps: 3n, stale_haircut_bps: 4n, print_policy: "Banded", band_bps: 5n, min_print_volume: 6n, min_print_investors: 7, outlier_band_bps: 8n, drills_enabled: false };
  assert.deepEqual(mark.decodeAt({ defined: { name: "FeedParams" } }, mark.encode({ defined: { name: "FeedParams" } }, p), 0)[0], { ...p, print_policy: "Banded" });
});
