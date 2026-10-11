/**
 * Access log for the gated pages (SRS 08 §7 Access, #202): who signed in,
 * who was refused and why, and who opened the data room or the admin
 * console, as one JSON line per event. Server-side only, append-only, no
 * external service.
 *
 * `NUNDINA_ACCESS_LOG=<path>` appends the lines to that file (the host's
 * log shipping or a cron picks it up); without it every line goes to
 * stdout as `[nundina-access] {...}`, which the hosting platform keeps.
 * Wallet addresses are public keys, not PII; the log never records a
 * signature, a cookie or a challenge nonce.
 */
import { appendFileSync } from "node:fs";
import { accessMode } from "./access";

export type AccessEvent =
  | "challenge"
  | "sign-in"
  | "sign-in-refused"
  | "sign-out"
  | "gated-view"
  | "gated-refused";

export interface AccessLine {
  at: string;
  event: AccessEvent;
  wallet: string | null;
  path: string;
  mode: "allowlist" | "open";
  /** Why the gate admitted or refused (no secrets). */
  detail?: string;
}

/** Append one access event; never throws (a log failure must not open or close a page). */
export function logAccess(event: AccessEvent, wallet: string | null, path: string, detail?: string): AccessLine {
  const line: AccessLine = { at: new Date().toISOString(), event, wallet, path, mode: accessMode(), ...(detail ? { detail } : {}) };
  const json = JSON.stringify(line);
  const file = process.env.NUNDINA_ACCESS_LOG;
  try {
    if (file) appendFileSync(file, json + "\n");
    else console.log(`[nundina-access] ${json}`);
  } catch (e) {
    console.error(`[nundina-access] log write failed: ${(e as Error).message} | ${json}`);
  }
  return line;
}

let warned = false;

/**
 * A production server in "open" mode serves every page but the data room
 * and admin to anyone: say so loudly, once per process.
 */
export function warnIfOpenInProduction(): boolean {
  if (warned || process.env.NODE_ENV !== "production" || accessMode() !== "open") return false;
  warned = true;
  console.warn(
    "[nundina-access] WARNING: NUNDINA_ACCESS is not 'allowlist' in a production build. " +
      "Every page except /data-room and /admin is readable without sign-in. Set NUNDINA_ACCESS=allowlist " +
      "and NUNDINA_ACCESS_SECRET for any deployment outside local development.",
  );
  return true;
}
