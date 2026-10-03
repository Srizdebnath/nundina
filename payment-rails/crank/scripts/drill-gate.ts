/**
 * credit-gate + SAS for the kill-the-crank drill (#133, #134). Every fill
 * CPIs the gate, so the drill's validator runs the real credit-gate (root
 * `anchor build`) and the real SAS program (the gate suite's pinned
 * fixture), and the drill goes through their own instructions: gate root →
 * market (config, policy, block-list) → SAS credential + schemas → identity
 * and accreditation attestations → `validate` per buyer.
 *
 * One thing can't happen on a fresh validator: inventory that has already
 * seasoned (≥ 1 day by the gate's own floor). So before the validator
 * starts, `fixtures <dir>` writes the sellers' keypairs, the asset mint
 * keypair, and each seller's `HolderLots` ledger holding one lot acquired
 * 30 days ago, as `--account` files (run-drill.sh). The layout is the one
 * `lots.rs` `layout_read_by_the_seasoning_crank` pins.
 *
 * Instruction encodings follow ops/gate/{sas,wire}.ts (Anchor sighash +
 * Borsh), which were exercised against the same programs.
 */
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  AccountRole,
  address,
  createKeyPairSignerFromBytes,
  getAddressEncoder,
  getProgramDerivedAddress,
  type Address,
  type Instruction,
  type KeyPairSigner,
  type TransactionSigner,
} from "@solana/kit";
import { send, type Ctx } from "../src/send.ts";
import { CREDIT_GATE } from "../src/seasoning.ts";

export const SAS = address("22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG");
const SYSTEM = address("11111111111111111111111111111111");
const LOADER_V3 = address("BPFLoaderUpgradeab1e11111111111111111111111");
const DAY = 86_400n;
export const SEASONING = 7n * DAY;
/** Age of the sellers' starting inventory: long seasoned. */
export const INVENTORY_AGE = 30n * DAY;

const utf8 = new TextEncoder();
const enc = getAddressEncoder();
/** An address's 32 bytes, mutable. */
const ab = (a: Address) => Uint8Array.from(enc.encode(a));
const bytes = (...parts: (Uint8Array | number[])[]) => Uint8Array.from(parts.flatMap((p) => Array.from(p)));
const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const u64 = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(n); return b; };
const i64 = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigInt64LE(n); return b; };
const str = (s: string | Uint8Array) => { const b = typeof s === "string" ? utf8.encode(s) : s; return bytes(u32(b.length), b); };
const sighash = (ns: string, name: string) => createHash("sha256").update(`${ns}:${name}`).digest().subarray(0, 8);

/** A seed: an address (32 bytes) or raw bytes; literals go through `L`. */
type Seed = Address | Uint8Array;
const L = (literal: string) => utf8.encode(literal);
async function pda(program: Address, ...seeds: Seed[]): Promise<readonly [Address, number]> {
  return getProgramDerivedAddress({
    programAddress: program,
    seeds: seeds.map((x) => (typeof x === "string" ? ab(x) : x)),
  });
}
const gatePda = async (...x: Seed[]) => (await pda(CREDIT_GATE, ...x))[0];
const sasPda = async (...x: Seed[]) => (await pda(SAS, ...x))[0];

type Meta = NonNullable<Instruction["accounts"]>[number];
const r = (a: Address): Meta => ({ address: a, role: AccountRole.READONLY });
const w = (a: Address): Meta => ({ address: a, role: AccountRole.WRITABLE });
const s = (k: TransactionSigner, writable = false): Meta =>
  ({ address: k.address, role: writable ? AccountRole.WRITABLE_SIGNER : AccountRole.READONLY_SIGNER, signer: k }) as Meta;
const ix = (programAddress: Address, accounts: Meta[], data: Uint8Array): Instruction => ({ programAddress, accounts, data });

async function must(ctx: Ctx, label: string, instructions: Instruction[]): Promise<void> {
  const res = await send(ctx, label, instructions);
  if (res.status !== "sent") throw new Error(`${label}: ${res.name ?? res.message}`);
}

/** A non-zero investor id unique to `wallet` (same rule as tests-svm). */
export function investorOf(wallet: Address): Uint8Array {
  const id = ab(wallet).slice(0, 16);
  id[0]! |= 1;
  return Uint8Array.from(id);
}

// ── pre-validator fixtures ───────────────────────────────────────────────

export const holderLotsPda = (mint: Address, holder: Address) => pda(CREDIT_GATE, L("lots"), mint, holder);

/** `HolderLots` bytes: one lot of `qty` acquired at `acquired`. 582 bytes. */
export function holderLotsData(mint: Address, holder: Address, qty: bigint, acquired: bigint, bump: number): Uint8Array {
  const lot = bytes(u32(0), u64(qty), i64(acquired), i64(acquired + SEASONING), [0, 0, 0]);
  const empty = new Uint8Array(31 * 15);
  const data = bytes(sighash("account", "HolderLots"), ab(mint), ab(holder), u64(0n), u32(1), [1], lot, empty, [bump]);
  if (data.length !== 582) throw new Error(`HolderLots is ${data.length} bytes, want 582`);
  return data;
}

async function newKeypairFile(path: string): Promise<KeyPairSigner> {
  const { generateKeyPairSync } = await import("node:crypto");
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const seed = privateKey.export({ format: "der", type: "pkcs8" }).subarray(-32);
  const pub = publicKey.export({ format: "der", type: "spki" }).subarray(-32);
  const secret = Uint8Array.from([...seed, ...pub]);
  writeFileSync(path, JSON.stringify(Array.from(secret)), { mode: 0o600 });
  return createKeyPairSignerFromBytes(secret);
}

export async function loadSigner(path: string): Promise<KeyPairSigner> {
  return createKeyPairSignerFromBytes(Uint8Array.from(JSON.parse(readFileSync(path, "utf8"))));
}

/**
 * `fixtures <dir> <qtyA> <qtyB>`: keypairs for the asset mint and sellers
 * A, B, and their seasoned ledgers as `--account` files. Prints the
 * validator flags.
 */
async function fixtures(dir: string, qtys: bigint[]): Promise<void> {
  mkdirSync(dir, { recursive: true });
  const mint = await newKeypairFile(join(dir, "asset-mint.json"));
  const flags: string[] = [];
  const acquired = BigInt(Math.floor(Date.now() / 1000)) - INVENTORY_AGE;
  for (const [i, qty] of qtys.entries()) {
    const seller = await newKeypairFile(join(dir, `seller-${i}.json`));
    const [lots, bump] = await holderLotsPda(mint.address, seller.address);
    const file = join(dir, `lots-${i}.json`);
    writeFileSync(file, JSON.stringify({
      pubkey: lots,
      account: {
        lamports: 10_000_000,
        data: [Buffer.from(holderLotsData(mint.address, seller.address, qty, acquired, bump)).toString("base64"), "base64"],
        owner: CREDIT_GATE,
        executable: false,
        rentEpoch: 0,
        space: 582,
      },
    }));
    flags.push("--account", lots, file);
  }
  console.log(flags.join(" "));
}

// ── on-validator setup ───────────────────────────────────────────────────

export interface GateMarket {
  mint: Address;
  attester: KeyPairSigner;
  credential: Address;
  idSchema: Address;
  accSchema: Address;
}

/**
 * Gate root (the drill authority is credit-gate's upgrade authority on the
 * validator), SAS credential + the two schemas, and the market for `mint`:
 * seasoning 7 days, horizon 1 day, cap 100, US / KYC ≥ 1 / accredited.
 */
export async function setupGateMarket(ctx: Ctx, authority: KeyPairSigner, mint: Address, attester: KeyPairSigner): Promise<GateMarket> {
  const root = await gatePda(L("gate_root"));
  const [programData] = await pda(LOADER_V3, ab(CREDIT_GATE));
  await must(ctx, "gate initialize_root", [
    ix(CREDIT_GATE, [w(root), s(authority, true), r(CREDIT_GATE), r(programData), r(SYSTEM)],
      bytes(sighash("global", "initialize_root"), ab(authority.address))),
  ]);

  const credential = await sasPda(L("credential"), authority.address, L("nundina"));
  await must(ctx, "SAS credential", [
    ix(SAS, [s(authority, true), w(credential), s(authority), r(SYSTEM)],
      bytes([0], str("nundina"), u32(1), ab(attester.address))),
  ]);
  const schema = (name: string) => pda(SAS, L("schema"), credential, L(name), Uint8Array.of(1)).then((x) => x[0]);
  const [idSchema, accSchema] = [await schema("nundina.identity.v1"), await schema("nundina.accreditation.v1")];
  for (const [name, addr, layout, fields] of [
    ["nundina.identity.v1", idSchema, [4, 12, 0], ["investor_id", "jurisdiction", "kyc_level"]],
    ["nundina.accreditation.v1", accSchema, [10, 0], ["accredited", "method"]],
  ] as const) {
    await must(ctx, `SAS schema ${name}`, [
      ix(SAS, [s(authority, true), s(authority), r(credential), w(addr), r(SYSTEM)],
        bytes([1], str(name), str("NUNDINA drill schema"), str(Uint8Array.from(layout)), u32(fields.length), ...fields.map((f) => str(f)))),
    ]);
  }

  const config = await gatePda(L("config"), mint);
  const policy = await gatePda(L("policy"), mint);
  await must(ctx, "gate market", [
    ix(CREDIT_GATE, [w(config), r(root), r(mint), s(authority, true), r(SYSTEM)], bytes(
      sighash("global", "initialize_config"), u32(1), u32(100), i64(SEASONING), i64(DAY),
      u32(1), ab(attester.address), ab(idSchema), ab(accSchema),
    )),
    ix(CREDIT_GATE, [r(config), w(policy), r(idSchema), r(accSchema), r(mint), s(authority, true), r(SYSTEM)], bytes(
      sighash("global", "initialize_policy"), ab(credential), u32(1), utf8.encode("US"), [1, 1],
    )),
    ix(CREDIT_GATE, [w(config), w(await gatePda(L("blocklist"), mint)), r(mint), s(authority, true), r(SYSTEM)],
      sighash("global", "initialize_blocklist")),
  ]);
  return { mint, attester, credential, idSchema, accSchema };
}

/** Registrar step + gate admission for `wallet`: SAS attests, then `validate`. */
export async function admit(ctx: Ctx, payer: KeyPairSigner, m: GateMarket, wallet: Address): Promise<void> {
  const expiry = BigInt(Math.floor(Date.now() / 1000)) + 90n * DAY;
  const attestation = (schema: Address) => sasPda(L("attestation"), m.credential, schema, wallet);
  const [idAtt, accAtt] = [await attestation(m.idSchema), await attestation(m.accSchema)];
  const identity = bytes(investorOf(wallet), str("US"), [2]);
  for (const [schema, att, data] of [[m.idSchema, idAtt, identity], [m.accSchema, accAtt, Uint8Array.of(1, 1)]] as const) {
    await must(ctx, "SAS attest", [
      ix(SAS, [s(payer, true), s(m.attester), r(m.credential), r(schema), w(att), r(SYSTEM)],
        bytes([6], ab(wallet), str(data), i64(expiry))),
    ]);
  }
  await must(ctx, "gate validate", [
    ix(CREDIT_GATE, [
      r(await gatePda(L("config"), m.mint)), w(await gatePda(L("policy"), m.mint)), r(await gatePda(L("blocked"), m.mint, wallet)),
      r(idAtt), r(accAtt), w(await gatePda(L("holder"), m.mint, wallet)), w(await gatePda(L("investor"), m.mint, investorOf(wallet))),
      w(await gatePda(L("lots"), m.mint, wallet)), r(m.mint), s(payer, true), r(SYSTEM),
    ], bytes(sighash("global", "validate"), ab(wallet))),
  ]);
}

/** Pure: a `HolderLots` account's FREE + lot quantities. */
export function ledgerTotal(data: Uint8Array): bigint {
  const v = new DataView(data.buffer, data.byteOffset, data.byteLength);
  let total = v.getBigUint64(72, true);
  for (let i = 0; i < data[84]!; i++) total += v.getBigUint64(85 + 31 * i + 4, true);
  return total;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [cmd, dir, ...qtys] = process.argv.slice(2);
  if (cmd !== "fixtures" || !dir || qtys.length === 0) {
    console.error("usage: drill-gate.ts fixtures <dir> <qty>...");
    process.exit(2);
  }
  await fixtures(dir, qtys.map(BigInt));
}
