/**
 * IDL-driven Borsh codec for the payment-rails program.
 *
 * Everything the crank encodes or decodes — instruction data, accounts,
 * events — goes through the program's own anchor IDL (`idl/payment_rails.json`,
 * synced from `anchor build` with `npm run sync-idl`), so the client can't
 * drift from the on-chain layout without the unit tests failing.
 *
 * Supported types are exactly what the programs use: u8/u16/u32/u64/i64,
 * u128/i128 (mark-engine NAVs), bool, pubkey, fixed arrays, vecs (u32 length
 * prefix), options (credit-gate / mark-engine events), structs and unit
 * enums. Values: u64/i64/u128 are `bigint`, pubkeys are `Address` strings,
 * `[u8; N]` is `Uint8Array`, `Vec<T>` is an array, `None` is `null`, enums
 * are their variant name.
 */
import { readFileSync } from "node:fs";
import {
  AccountRole,
  isTransactionSigner,
  address,
  getAddressDecoder,
  getAddressEncoder,
  type Address,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";

type IdlType =
  | "u8" | "u16" | "u32" | "u64" | "i64" | "u128" | "i128" | "bool" | "pubkey"
  | { array: [IdlType, number] }
  | { vec: IdlType }
  | { option: IdlType }
  | { defined: { name: string } };

interface IdlField { name: string; type: IdlType }
interface IdlTypeDef {
  name: string;
  type: { kind: "struct"; fields: IdlField[] } | { kind: "enum"; variants: { name: string }[] };
}
interface IdlAccountMeta {
  name: string;
  writable?: boolean;
  signer?: boolean;
  address?: string;
  /** A composite `Accounts` field (e.g. `refund_escalated.refund`, #127). */
  accounts?: IdlAccountMeta[];
}
interface IdlInstruction {
  name: string;
  discriminator: number[];
  accounts: IdlAccountMeta[];
  args: IdlField[];
}
export interface Idl {
  address: string;
  instructions: IdlInstruction[];
  accounts: { name: string; discriminator: number[] }[];
  events: { name: string; discriminator: number[] }[];
  errors: { code: number; name: string; msg?: string }[];
  types: IdlTypeDef[];
}

export type Value = bigint | number | boolean | string | null | Uint8Array | Value[] | { [k: string]: Value };
export type Struct = { [k: string]: Value };

const IDL_PATH = new URL("../idl/payment_rails.json", import.meta.url);
export const idl: Idl = JSON.parse(readFileSync(IDL_PATH, "utf8"));

/** An account argument: a plain address, or a signer for signing slots. */
export type AccountArg = Address | TransactionSigner | { [name: string]: AccountArg };

/**
 * A codec bound to one program's IDL. The module's own exports are the
 * payment-rails codec; the engine cranks (#149) build theirs from
 * `idl/auction_engine.json` etc. with the same rules.
 */
export function makeCodec(idl: Idl) {
const PROGRAM_ID: Address = address(idl.address);

const typeDefs = new Map(idl.types.map((t) => [t.name, t]));
const addrEnc = getAddressEncoder();
const addrDec = getAddressDecoder();

// ── encoding ─────────────────────────────────────────────────────────────

function encodeInto(t: IdlType, v: Value, out: number[]): void {
  switch (t) {
    case "u8": out.push(Number(v) & 0xff); return;
    case "bool": out.push(v ? 1 : 0); return;
    case "u16": pushLe(BigInt(v as number), 2, out); return;
    case "u32": pushLe(BigInt(v as number), 4, out); return;
    case "u64": pushLe(BigInt(v as bigint), 8, out); return;
    case "i64": pushLe(BigInt.asUintN(64, BigInt(v as bigint)), 8, out); return;
    case "u128": pushLe(BigInt(v as bigint), 16, out); return;
    case "i128": pushLe(BigInt.asUintN(128, BigInt(v as bigint)), 16, out); return;
    case "pubkey": out.push(...addrEnc.encode(v as Address)); return;
  }
  if ("option" in t) {
    if (v === null || v === undefined) { out.push(0); return; }
    out.push(1);
    encodeInto(t.option, v, out);
    return;
  }
  if ("array" in t) {
    const [inner, len] = t.array;
    const items = v as Uint8Array | Value[];
    if (items.length !== len) throw new Error(`array: expected ${len} items, got ${items.length}`);
    for (const item of items as Iterable<Value>) encodeInto(inner, item, out);
    return;
  }
  if ("vec" in t) {
    const items = v as Value[];
    pushLe(BigInt(items.length), 4, out);
    for (const item of items) encodeInto(t.vec, item, out);
    return;
  }
  const def = typeDefs.get(t.defined.name);
  if (!def) throw new Error(`unknown type ${t.defined.name}`);
  if (def.type.kind === "enum") {
    const i = def.type.variants.findIndex((x) => x.name === v);
    if (i < 0) throw new Error(`${def.name}: unknown variant ${String(v)}`);
    out.push(i);
    return;
  }
  for (const f of def.type.fields) {
    const fv = (v as Struct)[f.name];
    if (fv === undefined) throw new Error(`${def.name}.${f.name} missing`);
    encodeInto(f.type, fv, out);
  }
}

function pushLe(n: bigint, bytes: number, out: number[]): void {
  if (n < 0n || n >= 1n << BigInt(bytes * 8)) throw new Error(`integer out of range: ${n}`);
  for (let i = 0; i < bytes; i++) out.push(Number((n >> BigInt(8 * i)) & 0xffn));
}

function encode(t: IdlType, v: Value): Uint8Array {
  const out: number[] = [];
  encodeInto(t, v, out);
  return Uint8Array.from(out);
}

// ── decoding ─────────────────────────────────────────────────────────────

function readLe(b: Uint8Array, o: number, bytes: number): bigint {
  if (o + bytes > b.length) throw new Error("buffer too short");
  let n = 0n;
  for (let i = bytes - 1; i >= 0; i--) n = (n << 8n) | BigInt(b[o + i]!);
  return n;
}

function decodeAt(t: IdlType, b: Uint8Array, o: number): [Value, number] {
  switch (t) {
    case "u8": return [Number(readLe(b, o, 1)), o + 1];
    case "bool": return [readLe(b, o, 1) !== 0n, o + 1];
    case "u16": return [Number(readLe(b, o, 2)), o + 2];
    case "u32": return [Number(readLe(b, o, 4)), o + 4];
    case "u64": return [readLe(b, o, 8), o + 8];
    case "i64": return [BigInt.asIntN(64, readLe(b, o, 8)), o + 8];
    case "u128": return [readLe(b, o, 16), o + 16];
    case "i128": return [BigInt.asIntN(128, readLe(b, o, 16)), o + 16];
    case "pubkey": return [addrDec.decode(b.subarray(o, o + 32)), o + 32];
  }
  if ("option" in t) {
    const tag = Number(readLe(b, o, 1));
    if (tag === 0) return [null, o + 1];
    if (tag !== 1) throw new Error(`bad option tag ${tag}`);
    return decodeAt(t.option, b, o + 1);
  }
  if ("array" in t) {
    const [inner, len] = t.array;
    if (inner === "u8") return [b.slice(o, o + len), o + len];
    const items: Value[] = [];
    for (let i = 0; i < len; i++) {
      const [v, next] = decodeAt(inner, b, o);
      items.push(v);
      o = next;
    }
    return [items, o];
  }
  if ("vec" in t) {
    const len = Number(readLe(b, o, 4));
    o += 4;
    const items: Value[] = [];
    for (let i = 0; i < len; i++) {
      const [v, next] = decodeAt(t.vec, b, o);
      items.push(v);
      o = next;
    }
    return [items, o];
  }
  const def = typeDefs.get(t.defined.name);
  if (!def) throw new Error(`unknown type ${t.defined.name}`);
  if (def.type.kind === "enum") {
    const i = Number(readLe(b, o, 1));
    const variant = def.type.variants[i];
    if (!variant) throw new Error(`${def.name}: bad variant index ${i}`);
    return [variant.name, o + 1];
  }
  const s: Struct = {};
  for (const f of def.type.fields) {
    const [v, next] = decodeAt(f.type, b, o);
    s[f.name] = v;
    o = next;
  }
  return [s, o];
}

const eqDisc = (b: Uint8Array, d: number[]) => d.every((x, i) => b[i] === x);

/** Decode a program account; `null` if the discriminator isn't `name`'s. */
function decodeAccount<T = Struct>(name: string, data: Uint8Array): T | null {
  const acc = idl.accounts.find((a) => a.name === name);
  if (!acc) throw new Error(`unknown account ${name}`);
  if (data.length < 8 || !eqDisc(data, acc.discriminator)) return null;
  return decodeAt({ defined: { name } }, data, 8)[0] as T;
}

function accountDiscriminator(name: string): Uint8Array {
  const acc = idl.accounts.find((a) => a.name === name);
  if (!acc) throw new Error(`unknown account ${name}`);
  return Uint8Array.from(acc.discriminator);
}

/** Decode one `Program data:` payload into `{ name, data }`, or `null`. */
function decodeEvent(payload: Uint8Array): { name: string; data: Struct } | null {
  for (const ev of idl.events) {
    if (payload.length >= 8 && eqDisc(payload, ev.discriminator)) {
      return { name: ev.name, data: decodeAt({ defined: { name: ev.name } }, payload, 8)[0] as Struct };
    }
  }
  return null;
}

function errorName(code: number): string | undefined {
  return idl.errors.find((e) => e.code === code)?.name;
}

function errorCode(name: string): number {
  const e = idl.errors.find((x) => x.name === name);
  if (!e) throw new Error(`unknown error ${name}`);
  return e.code;
}

// ── instructions ─────────────────────────────────────────────────────────

const isSigner = (a: AccountArg): a is TransactionSigner =>
  typeof a === "object" && isTransactionSigner(a as TransactionSigner);

/**
 * Build a program instruction by IDL name. Accounts are matched by IDL name
 * (fixed-address accounts such as `system_program` are filled in); roles
 * come from the IDL's `writable` / `signer` flags, so a caller can't get
 * them wrong.
 */
function ix(name: string, accounts: Record<string, AccountArg>, args: Struct = {}): Instruction {
  const def = idl.instructions.find((i) => i.name === name);
  if (!def) throw new Error(`unknown instruction ${name}`);
  // Composite fields nest: `{ refund: { caller, … }, keeper: { … } }`,
  // flattened in IDL order, exactly as Anchor lays them out on the wire.
  type Meta = NonNullable<Instruction["accounts"]>[number];
  const flatten = (defs: IdlAccountMeta[], given: Record<string, AccountArg>, path: string): Meta[] =>
    defs.flatMap((a) => {
      const v = given[a.name] ?? (a.address ? address(a.address) : undefined);
      if (v === undefined) throw new Error(`${name}: missing account ${path}${a.name}`);
      if (a.accounts) {
        if (typeof v === "string" || isSigner(v)) throw new Error(`${name}: ${path}${a.name} is a group of accounts`);
        return flatten(a.accounts, v as Record<string, AccountArg>, `${path}${a.name}.`);
      }
      if (typeof v !== "string" && !isSigner(v)) throw new Error(`${name}: ${path}${a.name} is a single account`);
      if (a.signer && !isSigner(v)) throw new Error(`${name}: ${a.name} must be a signer`);
      const role = a.signer
        ? a.writable ? AccountRole.WRITABLE_SIGNER : AccountRole.READONLY_SIGNER
        : a.writable ? AccountRole.WRITABLE : AccountRole.READONLY;
      return [isSigner(v) ? { address: v.address, role, signer: v } : { address: v, role }];
    });
  const metas = flatten(def.accounts, accounts, "");
  const data: number[] = [...def.discriminator];
  for (const arg of def.args) {
    const v = args[arg.name];
    if (v === undefined) throw new Error(`${name}: missing arg ${arg.name}`);
    encodeInto(arg.type, v, data);
  }
  return { programAddress: PROGRAM_ID, accounts: metas, data: Uint8Array.from(data) };
}

return {
  idl,
  programId: PROGRAM_ID,
  encode,
  decodeAt,
  decodeAccount,
  accountDiscriminator,
  decodeEvent,
  errorName,
  errorCode,
  ix,
};
}

const rails = makeCodec(idl);
export const PROGRAM_ID: Address = rails.programId;
export const encode = rails.encode;
export const decodeAt = rails.decodeAt;
export const decodeAccount: <T = Struct>(name: string, data: Uint8Array) => T | null = rails.decodeAccount;
export const accountDiscriminator = rails.accountDiscriminator;
export const decodeEvent = rails.decodeEvent;
export const errorName = rails.errorName;
export const errorCode = rails.errorCode;
export const ix = rails.ix;

/** Load another program's IDL from `idl/<name>.json`. */
export function loadIdl(name: string): Idl {
  return JSON.parse(readFileSync(new URL(`../idl/${name}.json`, import.meta.url), "utf8"));
}
