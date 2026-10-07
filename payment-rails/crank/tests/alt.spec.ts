// Lookup tables for wide crank steps: the auction `settle` (rails
// settle_fill + gate settle_check, ~30 accounts) is over the packet limit as
// a plain v0 transaction. The table instructions are hand-encoded, so pin
// their layout and the PDA; the end-to-end run is app/scripts/localnet-e2e.mts.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  AccountRole,
  address,
  appendTransactionMessageInstructions,
  compileTransaction,
  compressTransactionMessageUsingAddressLookupTables,
  createNoopSigner,
  createTransactionMessage,
  getAddressEncoder,
  generateKeyPairSigner,
  getProgramDerivedAddress,
  getTransactionEncoder,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  type Address,
  type Blockhash,
  type Instruction,
} from "@solana/kit";
import {
  ALT_PROGRAM,
  createLookupTableIx,
  extendLookupTableIx,
  lookupTablePda,
  tableCandidates,
} from "../src/alt.ts";

const A = address("9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ");
const B = address("8wj1EfUyfNvBaMhxBd2iuyC7RLQh9oW2AAQdNRzdXLCV");

test("table PDA is [authority, recent_slot LE] under the ALT program", async () => {
  const slot = 123_456n;
  const [pda, bump] = await lookupTablePda(A, slot);
  const le = new Uint8Array(8);
  new DataView(le.buffer).setBigUint64(0, slot, true);
  const [expect, eb] = await getProgramDerivedAddress({
    programAddress: ALT_PROGRAM,
    seeds: [getAddressEncoder().encode(A), le],
  });
  assert.equal(pda, expect);
  assert.equal(bump, eb);
});

test("create = u32 0 | u64 slot | u8 bump; extend = u32 2 | u64 n | n × 32", async () => {
  const [table, bump] = await lookupTablePda(A, 7n);
  const c = createLookupTableIx(table, A, A, 7n, bump);
  assert.deepEqual([...c.data!], [0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, bump]);
  assert.deepEqual(
    c.accounts!.map((m) => m.role),
    [AccountRole.WRITABLE, AccountRole.READONLY_SIGNER, AccountRole.WRITABLE_SIGNER, AccountRole.READONLY],
  );
  const e = extendLookupTableIx(table, A, A, [A, B]);
  assert.equal(e.data!.length, 4 + 8 + 64);
  assert.deepEqual([...e.data!.subarray(0, 12)], [2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
});

test("only accounts that never sign go in the table", () => {
  const P = address("Cw6wNpVJUaofUDZewG8sDKZu3nnHga4GaJJvKn7rSod7");
  const ix: Instruction = {
    programAddress: P,
    accounts: [
      { address: A, role: AccountRole.WRITABLE_SIGNER },
      { address: B, role: AccountRole.WRITABLE },
      { address: A, role: AccountRole.READONLY }, // also signs above: stays static
    ],
  };
  assert.deepEqual(tableCandidates([ix]), [B]);
});

test("a 36-account step fits the packet limit only through the table", async () => {
  const payer = createNoopSigner(A);
  const accts: Address[] = [];
  for (let i = 0; i < 36; i++) accts.push((await generateKeyPairSigner()).address);
  const P = address("Cw6wNpVJUaofUDZewG8sDKZu3nnHga4GaJJvKn7rSod7");
  const ix: Instruction = { programAddress: P, accounts: accts.map((a) => ({ address: a, role: AccountRole.WRITABLE })), data: new Uint8Array(16) };
  const base = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(payer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash({ blockhash: "11111111111111111111111111111111" as Blockhash, lastValidBlockHeight: 0n }, m),
    (m) => appendTransactionMessageInstructions([ix], m),
  );
  const size = (m: Parameters<typeof compileTransaction>[0]) => getTransactionEncoder().encode(compileTransaction(m)).length;
  const plain = size(base);
  assert.ok(plain > 1232, `plain: ${plain} bytes`);
  const packed = size(compressTransactionMessageUsingAddressLookupTables(base, { [B]: tableCandidates([ix]) }) as Parameters<typeof compileTransaction>[0]);
  assert.ok(packed <= 1232, `with table: ${packed} bytes`);
});
