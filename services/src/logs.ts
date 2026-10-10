/**
 * Anchor events out of a transaction's logs, attributed to the program that
 * emitted them.
 *
 * `emit!` writes `Program data: <base64>` while that program is executing,
 * so the emitter is the top of the invoke stack (`Program X invoke [n]` …
 * `Program X success|failed`). Attribution matters: auction-engine and
 * payment-rails both emit a `FillSettled`, with the same discriminator
 * (`sha256("event:FillSettled")`), in the same transaction (the engine CPIs
 * the rails).
 */
import type { Address } from "@solana/kit";
import { fromBase64, prefixed, type Struct } from "./codec.ts";
import { BY_ID, type ProgramName } from "./programs.ts";

export interface DecodedEvent {
  /** Position among the transaction's `Program data` lines. */
  index: number;
  program: ProgramName;
  programId: Address;
  name: string;
  data: Struct;
  /**
   * The bytes matched an event discriminator but not the IDL layout (a
   * deployment older or newer than our IDL — devnet payment-rails predates
   * #89's `EscrowEvent.unit`). Kept raw, never projected.
   */
  undecoded?: string;
}

const INVOKE = /^Program (\w{32,44}) invoke \[\d+\]$/;
const EXIT = /^Program (\w{32,44}) (success|failed)/;

export function eventsFromLogs(logs: readonly string[]): DecodedEvent[] {
  const stack: Address[] = [];
  const out: DecodedEvent[] = [];
  let index = 0;
  for (const line of logs) {
    const inv = INVOKE.exec(line);
    if (inv) {
      stack.push(inv[1] as Address);
      continue;
    }
    if (EXIT.test(line)) {
      stack.pop();
      continue;
    }
    if (!line.startsWith("Program data: ")) continue;
    const at = index++;
    const top = stack[stack.length - 1];
    const prog = top ? BY_ID.get(top) : undefined;
    if (!prog) continue;
    const bytes = fromBase64(line.slice("Program data: ".length));
    const ev = (prog.codec.idl.events ?? []).find((e) => prefixed(bytes, e.discriminator));
    if (!ev) continue;
    try {
      out.push({ index: at, program: prog.name, programId: top!, name: ev.name, data: prog.codec.decodeEvent(ev.name, bytes.subarray(8)) });
    } catch {
      out.push({ index: at, program: prog.name, programId: top!, name: ev.name, data: {}, undecoded: line.slice("Program data: ".length) });
    }
  }
  return out;
}

/** JSON-safe form of decoded values: bigint → decimal string, bytes → hex. */
export function plain(v: unknown): unknown {
  if (typeof v === "bigint") return v.toString();
  if (v instanceof Uint8Array) return Array.from(v, (b) => b.toString(16).padStart(2, "0")).join("");
  if (Array.isArray(v)) return v.map(plain);
  if (v && typeof v === "object") return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, plain(x)]));
  return v;
}
