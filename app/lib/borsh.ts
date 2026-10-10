/**
 * IDL-driven Borsh decoding for every nundina program the UI reads.
 *
 * Accounts and events are decoded with the program's own anchor IDL
 * (`app/idl/*.json`, synced from `anchor build` with `npm run sync-idl`), so
 * the UI cannot drift from the on-chain layout without `npm test` failing.
 *
 * Values: u64/i64/u128/i128 are `bigint`, u8–u32 are `number`, pubkeys are
 * base58 strings, `[u8; N]` is `Uint8Array`, options are the value or
 * `null`, unit enums are their variant name, enums with fields are
 * `{ [variant]: fields }`.
 */
import { getAddressDecoder } from "@solana/kit";

export type IdlType =
  | "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "u64" | "i64" | "u128" | "i128"
  | "bool" | "pubkey" | "string" | "bytes"
  | { array: [IdlType, number] }
  | { vec: IdlType }
  | { option: IdlType }
  | { defined: { name: string } };

export interface IdlField { name: string; type: IdlType }
export type IdlTypeDef = {
  name: string;
  type:
    | { kind: "struct"; fields?: IdlField[] | IdlType[] }
    | { kind: "enum"; variants: { name: string; fields?: IdlField[] | IdlType[] }[] };
};
export interface IdlAccountMeta {
  name: string;
  writable?: boolean;
  signer?: boolean;
  address?: string;
  accounts?: IdlAccountMeta[];
}
export interface Idl {
  address: string;
  metadata?: { name: string };
  instructions: { name: string; discriminator: number[]; accounts: IdlAccountMeta[]; args: IdlField[] }[];
  accounts?: { name: string; discriminator: number[] }[];
  events?: { name: string; discriminator: number[] }[];
  errors?: { code: number; name: string; msg?: string }[];
  types?: IdlTypeDef[];
}

export type Value =
  | bigint | number | boolean | string | null | Uint8Array | Value[] | { [k: string]: Value };
export type Struct = { [k: string]: Value };

const addressDecoder = getAddressDecoder();

class Reader {
  off = 0;
  constructor(readonly b: Uint8Array) {}
  private view = (n: number) => {
    if (this.off + n > this.b.length) throw new RangeError(`read past end (${this.off}+${n} > ${this.b.length})`);
    const v = new DataView(this.b.buffer, this.b.byteOffset + this.off, n);
    this.off += n;
    return v;
  };
  u8 = () => this.view(1).getUint8(0);
  i8 = () => this.view(1).getInt8(0);
  u16 = () => this.view(2).getUint16(0, true);
  i16 = () => this.view(2).getInt16(0, true);
  u32 = () => this.view(4).getUint32(0, true);
  i32 = () => this.view(4).getInt32(0, true);
  u64 = () => this.view(8).getBigUint64(0, true);
  i64 = () => this.view(8).getBigInt64(0, true);
  u128 = () => {
    const v = this.view(16);
    return v.getBigUint64(0, true) | (v.getBigUint64(8, true) << 64n);
  };
  i128 = () => {
    const v = this.view(16);
    return v.getBigUint64(0, true) | (v.getBigInt64(8, true) << 64n);
  };
  bytes = (n: number) => {
    if (this.off + n > this.b.length) throw new RangeError("read past end");
    const out = this.b.slice(this.off, this.off + n);
    this.off += n;
    return out;
  };
}

export class Codec {
  readonly types: Map<string, IdlTypeDef>;
  constructor(readonly idl: Idl) {
    this.types = new Map((idl.types ?? []).map((t) => [t.name, t]));
  }

  private read(r: Reader, t: IdlType): Value {
    if (typeof t === "string") {
      switch (t) {
        case "u8": return r.u8();
        case "i8": return r.i8();
        case "u16": return r.u16();
        case "i16": return r.i16();
        case "u32": return r.u32();
        case "i32": return r.i32();
        case "u64": return r.u64();
        case "i64": return r.i64();
        case "u128": return r.u128();
        case "i128": return r.i128();
        case "bool": return r.u8() !== 0;
        case "pubkey": return addressDecoder.decode(r.bytes(32));
        case "string": return new TextDecoder().decode(r.bytes(r.u32()));
        case "bytes": return r.bytes(r.u32());
      }
    }
    if ("array" in t) {
      const [inner, n] = t.array;
      if (inner === "u8") return r.bytes(n);
      return Array.from({ length: n }, () => this.read(r, inner));
    }
    if ("vec" in t) {
      const n = r.u32();
      if (t.vec === "u8") return r.bytes(n);
      return Array.from({ length: n }, () => this.read(r, t.vec));
    }
    if ("option" in t) return r.u8() === 0 ? null : this.read(r, t.option);
    return this.readDefined(r, t.defined.name);
  }

  private readFields(r: Reader, fields: IdlField[] | IdlType[] | undefined): Value {
    if (!fields || fields.length === 0) return {};
    if (typeof fields[0] === "object" && fields[0] !== null && "name" in fields[0]) {
      const out: Struct = {};
      for (const f of fields as IdlField[]) out[f.name] = this.read(r, f.type);
      return out;
    }
    return (fields as IdlType[]).map((t) => this.read(r, t));
  }

  private readDefined(r: Reader, name: string): Value {
    const def = this.types.get(name);
    if (!def) throw new Error(`${this.idl.metadata?.name ?? "idl"}: unknown type ${name}`);
    if (def.type.kind === "struct") return this.readFields(r, def.type.fields);
    const tag = r.u8();
    const v = def.type.variants[tag];
    if (!v) throw new Error(`${name}: bad enum tag ${tag}`);
    if (!v.fields || v.fields.length === 0) return v.name;
    return { [v.name]: this.readFields(r, v.fields) };
  }

  /** Decode `data` as type `name` (no discriminator). */
  decodeType(name: string, data: Uint8Array): Struct {
    return this.readDefined(new Reader(data), name) as Struct;
  }

  /**
   * Decode event `name`'s payload (no discriminator). Event fields are
   * append-only: an event logged by an older build ends before the fields
   * appended since, which are then absent from the result.
   */
  decodeEvent(name: string, data: Uint8Array): Struct {
    const def = this.types.get(name);
    if (!def || def.type.kind !== "struct") throw new Error(`event ${name} has no struct type`);
    const r = new Reader(data);
    const out: Struct = {};
    for (const f of (def.type.fields ?? []) as IdlField[]) {
      if (r.off === data.length) break;
      out[f.name] = this.read(r, f.type);
    }
    return out;
  }

  /** Which account type `data` holds, by its 8-byte discriminator. */
  accountName(data: Uint8Array): string | null {
    for (const a of this.idl.accounts ?? []) if (prefixed(data, a.discriminator)) return a.name;
    return null;
  }

  /** Decode an account; throws if the discriminator is not `name`'s. */
  decodeAccount(name: string, data: Uint8Array): Struct {
    const a = this.idl.accounts?.find((x) => x.name === name);
    if (!a) throw new Error(`no account ${name}`);
    if (!prefixed(data, a.discriminator)) throw new Error(`not a ${name} account`);
    return this.decodeType(name, data.subarray(8));
  }

  /** Every anchor event in a transaction's logs (`Program data: <b64>`). */
  eventsFromLogs(logs: readonly string[]): { name: string; data: Struct }[] {
    const out: { name: string; data: Struct }[] = [];
    for (const l of logs) {
      if (!l.startsWith("Program data: ")) continue;
      const bytes = fromBase64(l.slice("Program data: ".length));
      for (const e of this.idl.events ?? []) {
        if (prefixed(bytes, e.discriminator)) {
          out.push({ name: e.name, data: this.decodeEvent(e.name, bytes.subarray(8)) });
          break;
        }
      }
    }
    return out;
  }

  /** Error name for a custom program error code, if this IDL defines it. */
  errorName(code: number): string | null {
    return this.idl.errors?.find((e) => e.code === code)?.name ?? null;
  }

  discriminator(account: string): Uint8Array {
    const a = this.idl.accounts?.find((x) => x.name === account);
    if (!a) throw new Error(`no account ${account}`);
    return Uint8Array.from(a.discriminator);
  }
}

export function prefixed(data: Uint8Array, d: number[]): boolean {
  if (data.length < d.length) return false;
  for (let i = 0; i < d.length; i++) if (data[i] !== d[i]) return false;
  return true;
}

export function fromBase64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function toBase64(b: Uint8Array): string {
  let s = "";
  for (const x of b) s += String.fromCharCode(x);
  return btoa(s);
}

export function u32le(n: number): Uint8Array {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n, true);
  return b;
}

export function u16le(n: number): Uint8Array {
  const b = new Uint8Array(2);
  new DataView(b.buffer).setUint16(0, n, true);
  return b;
}

export function u64le(n: bigint): Uint8Array {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigUint64(0, n, true);
  return b;
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((s, p) => s + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

export const hex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
export function fromHex(s: string): Uint8Array {
  if (!/^([0-9a-f]{2})*$/i.test(s)) throw new Error("bad hex");
  return Uint8Array.from(s.match(/../g) ?? [], (h) => parseInt(h, 16));
}
