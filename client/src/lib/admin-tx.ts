/**
 * Administrative instructions (SRS 08 §7 `/admin`), built from the IDLs'
 * discriminators and account orders. The page signs one with the connected
 * wallet only when that wallet *is* the authority; when the authority is the
 * Squads vault the instruction is exported for a vault proposal instead
 * (SRS 08 §8: admin actions are multisig-proposed, 48 h timelocked).
 */
import { AccountRole, getAddressEncoder, type Address, type Instruction } from "@solana/kit";
import { concat, toBase64, u16le, type Idl } from "./borsh";
import { codecs, pdas, PROGRAMS, SYSTEM_PROGRAM } from "./programs";

const enc = getAddressEncoder();
const disc = (idl: Idl, name: string) => {
  const i = idl.instructions.find((x) => x.name === name);
  if (!i) throw new Error(`no instruction ${name} in ${idl.address}`);
  return Uint8Array.from(i.discriminator);
};
const ro = (address: Address) => ({ address, role: AccountRole.READONLY });
const w = (address: Address) => ({ address, role: AccountRole.WRITABLE });
const signer = (address: Address, writable = false) => ({
  address,
  role: writable ? AccountRole.WRITABLE_SIGNER : AccountRole.READONLY_SIGNER,
});
const key = (a: Address) => enc.encode(a) as Uint8Array;
const bool = (b: boolean) => Uint8Array.of(b ? 1 : 0);

/** sha256 of an operator's note: the `evidence_hash` / `reason_hash` the programs event. */
export async function noteHash(note: string): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(note)));
}

export const ROLE = { sessions: 1, facility: 2, coupon: 4 } as const;

export const adminIx = {
  /** payment-rails `set_breaker(Pause | Depeg, engaged, evidence_hash)`. */
  async setBreaker(authority: Address, breaker: "Pause" | "Depeg", engaged: boolean, evidence: Uint8Array): Promise<Instruction> {
    return {
      programAddress: PROGRAMS.rails,
      accounts: [w(await pdas.railsConfig()), signer(authority)],
      data: concat(disc(codecs.rails.idl, "set_breaker"), Uint8Array.of(breaker === "Pause" ? 0 : 1), bool(engaged), evidence),
    };
  },
  /** payment-rails `set_fees(fee_bps, reserve_bps)`; the program caps both (fee at 0, doc 07 AE-3). */
  async setFees(authority: Address, feeBps: number, reserveBps: number): Promise<Instruction> {
    return {
      programAddress: PROGRAMS.rails,
      accounts: [w(await pdas.railsConfig()), signer(authority)],
      data: concat(disc(codecs.rails.idl, "set_fees"), u16le(feeBps), u16le(reserveBps)),
    };
  },
  /** payment-rails `grant_engine(engine, roles)`: admit an engine's signer PDA as operator for `roles`. */
  async grantEngine(authority: Address, engine: Address, roles: number): Promise<Instruction> {
    return {
      programAddress: PROGRAMS.rails,
      accounts: [ro(await pdas.railsConfig()), w(await pdas.engineGrant(engine)), signer(authority, true), ro(SYSTEM_PROGRAM)],
      data: concat(disc(codecs.rails.idl, "grant_engine"), key(engine), Uint8Array.of(roles)),
    };
  },
  /** mark-engine `suspend(reason_hash)` / `resume`. */
  async suspendFeed(authority: Address, mint: Address, reason: Uint8Array): Promise<Instruction> {
    return {
      programAddress: PROGRAMS.mark,
      accounts: [w(await pdas.feed(mint)), signer(authority)],
      data: concat(disc(codecs.mark.idl, "suspend"), reason),
    };
  },
  async resumeFeed(authority: Address, mint: Address): Promise<Instruction> {
    return {
      programAddress: PROGRAMS.mark,
      accounts: [w(await pdas.feed(mint)), signer(authority)],
      data: disc(codecs.mark.idl, "resume"),
    };
  },
  /** credit-gate `block_wallet(wallet, reason_hash)` / `unblock_wallet(wallet)`. */
  async blockWallet(authority: Address, mint: Address, wallet: Address, reason: Uint8Array): Promise<Instruction> {
    return {
      programAddress: PROGRAMS.gate,
      accounts: [
        ro(await pdas.gateConfig(mint)),
        w(await pdas.blocklist(mint)),
        w(await pdas.blocked(mint, wallet)),
        ro(mint),
        signer(authority, true),
        ro(SYSTEM_PROGRAM),
      ],
      data: concat(disc(codecs.gate.idl, "block_wallet"), key(wallet), reason),
    };
  },
  async unblockWallet(authority: Address, mint: Address, wallet: Address): Promise<Instruction> {
    return {
      programAddress: PROGRAMS.gate,
      accounts: [
        ro(await pdas.gateConfig(mint)),
        w(await pdas.blocklist(mint)),
        w(await pdas.blocked(mint, wallet)),
        ro(mint),
        signer(authority, true),
      ],
      data: concat(disc(codecs.gate.idl, "unblock_wallet"), key(wallet)),
    };
  },
};

const ROLE_NAME: Record<number, string> = {
  [AccountRole.READONLY]: "readonly",
  [AccountRole.WRITABLE]: "writable",
  [AccountRole.READONLY_SIGNER]: "signer",
  [AccountRole.WRITABLE_SIGNER]: "writable signer",
};

/**
 * The instruction as a Squads vault transaction needs it: program, ordered
 * accounts with their roles, data. `ops/squads` (or the Squads UI's
 * transaction builder) wraps it in a vault proposal; the authority account
 * is the vault itself, which signs on execution after the timelock.
 */
export function proposalJson(ix: Instruction): string {
  return JSON.stringify(
    {
      programId: ix.programAddress,
      accounts: (ix.accounts ?? []).map((a) => ({ pubkey: a.address, role: ROLE_NAME[a.role] })),
      data: toBase64(Uint8Array.from(ix.data ?? [])),
    },
    null,
    2,
  );
}
