import assert from "node:assert/strict";
import { test } from "node:test";
import { AccountRole, address, generateKeyPairSigner } from "@solana/kit";
import { accountDiscriminator, decodeAccount, decodeAt, encode, errorCode, errorName, idl, ix } from "../src/idl.ts";

const A = address("GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn");
const B = address("7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c");

test("settle_fill data = discriminator + u32 + u64 + u64 + [u8;32]", async () => {
  const op = await generateKeyPairSigner();
  const accounts = Object.fromEntries(
    idl.instructions.find((i) => i.name === "settle_fill")!.accounts.map((a) => [a.name, a.signer ? op : A]),
  );
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
