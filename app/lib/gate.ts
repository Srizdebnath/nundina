/**
 * The gate-rejection view (#144): why `credit-gate` admits or rejects a
 * wallet for a mint.
 *
 * The verdict is not recomputed here. The page *simulates the real
 * `validate` instruction* against the cluster and decodes the program's own
 * `ValidateDecision` event (or its typed error). The inputs it read — block
 * entry, SAS attestations, market policy, holder link, lot ledger, token
 * account state — are shown next to it so the reason can be traced.
 */
import {
  AccountRole,
  appendTransactionMessageInstruction,
  compileTransaction,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  pipe,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  type Address,
  type Instruction,
} from "@solana/kit";
import { concat, hex, type Struct } from "./borsh";
import { codecs, pdas, PROGRAMS, SYSTEM_PROGRAM } from "./programs";
import { getAccounts, getTokenHoldings, rpc, type RawAccount } from "./rpc";
import { parseAccreditation, parseAttestation, parseIdentity, type Attestation } from "./sas";
import { getAddressEncoder } from "@solana/kit";

export interface MarketView {
  config: Struct;
  policy: Struct;
  jurisdictions: string[];
}

export interface LotView {
  lotId: number;
  qty: bigint;
  acquiredTs: number;
  restrictionExpiry: number;
  source: string;
}

export interface Decision {
  /** `null` = approved. */
  reject: string | null;
  /** A typed program error instead of an evented decision. */
  error: string | null;
  distinctInvestors: number | null;
  logs: string[];
  payer: Address | null;
}

export interface GateStatus {
  mint: Address;
  wallet: Address;
  market: MarketView | null;
  blocked: Struct | null;
  identity: { address: Address; att: Attestation | null; parsed: ReturnType<typeof parseIdentity> };
  accreditation: { address: Address; att: Attestation | null; parsed: ReturnType<typeof parseAccreditation> };
  link: Struct | null;
  lots: { freeQty: bigint; lots: LotView[] } | null;
  tokenAccounts: { account: Address; amount: bigint; frozen: boolean }[];
  decision: Decision;
}

const enc = getAddressEncoder();

export function lotsView(raw: Struct): { freeQty: bigint; lots: LotView[] } {
  const n = raw.lot_count as number;
  const lots = (raw.lots as Struct[]).slice(0, n).map((l) => ({
    lotId: l.lot_id as number,
    qty: l.qty as bigint,
    acquiredTs: Number(l.acquired_ts as bigint),
    restrictionExpiry: Number(l.restriction_expiry as bigint),
    source: l.source as string,
  }));
  return { freeQty: raw.free_qty as bigint, lots };
}

function decodeIf(raw: RawAccount | null, owner: Address, name: string): Struct | null {
  if (!raw || raw.owner !== owner) return null;
  try {
    return codecs.gate.decodeAccount(name, raw.data);
  } catch {
    return null;
  }
}

export async function validateIx(
  mint: Address,
  wallet: Address,
  payer: Address,
  identity: Address,
  accreditation: Address,
  investorId: Uint8Array,
): Promise<Instruction> {
  const disc = codecs.gate.idl.instructions.find((i) => i.name === "validate")!.discriminator;
  const ro = (address: Address) => ({ address, role: AccountRole.READONLY });
  const w = (address: Address) => ({ address, role: AccountRole.WRITABLE });
  return {
    programAddress: PROGRAMS.gate,
    accounts: [
      ro(await pdas.gateConfig(mint)),
      w(await pdas.gatePolicy(mint)),
      ro(await pdas.blocked(mint, wallet)),
      ro(identity),
      ro(accreditation),
      w(await pdas.holderLink(mint, wallet)),
      w(await pdas.investor(mint, investorId)),
      w(await pdas.lots(mint, wallet)),
      ro(mint),
      { address: payer, role: AccountRole.WRITABLE_SIGNER },
      ro(SYSTEM_PROGRAM),
    ],
    data: concat(Uint8Array.from(disc), enc.encode(wallet) as Uint8Array),
  };
}

/** Simulate `validate` (no signature, nothing lands) and decode the verdict. */
export async function simulateValidate(ix: Instruction, payer: Address): Promise<Decision> {
  const { value: bh } = await rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
  const msg = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayer(payer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(bh, m),
    (m) => appendTransactionMessageInstruction(ix, m),
  );
  const wire = getBase64EncodedWireTransaction(compileTransaction(msg));
  const { value } = await rpc
    .simulateTransaction(wire, { encoding: "base64", sigVerify: false, replaceRecentBlockhash: true, commitment: "confirmed" })
    .send();
  const logs = [...(value.logs ?? [])];
  const ev = codecs.gate.eventsFromLogs(logs).find((e) => e.name === "ValidateDecision");
  if (ev) {
    return {
      reject: ev.data.reject === null ? null : String(ev.data.reject),
      error: null,
      distinctInvestors: ev.data.distinct_investors as number,
      logs,
      payer,
    };
  }
  return { reject: null, error: describeError(value.err), distinctInvestors: null, logs, payer };
}

/** A simulation error as the program's own error name where there is one. */
export function describeError(err: unknown): string {
  if (err === null || err === undefined) return "no decision event (unexpected)";
  const e = err as { InstructionError?: [number, { Custom?: number | bigint } | string] };
  const inner = e.InstructionError?.[1];
  if (inner && typeof inner === "object" && inner.Custom !== undefined) {
    const code = Number(inner.Custom);
    return codecs.gate.errorName(code) ?? `custom error ${code}`;
  }
  return JSON.stringify(err, (_k, v) => (typeof v === "bigint" ? v.toString() : v));
}

export async function gateStatus(mint: Address, wallet: Address): Promise<GateStatus> {
  const [configA, policyA, blockedA, linkA, lotsA] = await Promise.all([
    pdas.gateConfig(mint),
    pdas.gatePolicy(mint),
    pdas.blocked(mint, wallet),
    pdas.holderLink(mint, wallet),
    pdas.lots(mint, wallet),
  ]);
  const [configR, policyR, blockedR, linkR, lotsR, walletR] = await getAccounts([
    configA, policyA, blockedA, linkA, lotsA, wallet,
  ]);
  const config = decodeIf(configR, PROGRAMS.gate, "MarketConfig");
  const policy = decodeIf(policyR, PROGRAMS.gate, "MarketPolicy");
  const market: MarketView | null =
    config && policy
      ? {
          config,
          policy,
          jurisdictions: (policy.jurisdictions as Uint8Array[])
            .slice(0, policy.jurisdiction_count as number)
            .map((j) => String.fromCharCode(...j)),
        }
      : null;

  let identityA = wallet;
  let accreditationA = wallet;
  let identityAtt: Attestation | null = null;
  let accreditationAtt: Attestation | null = null;
  if (market) {
    const credential = market.policy.credential as Address;
    [identityA, accreditationA] = await Promise.all([
      pdas.attestation(credential, market.config.identity_schema as Address, wallet),
      pdas.attestation(credential, market.config.accreditation_schema as Address, wallet),
    ]);
    const [i, a] = await getAccounts([identityA, accreditationA]);
    identityAtt = i ? parseAttestation(i.data) : null;
    accreditationAtt = a ? parseAttestation(a.data) : null;
  }
  const identity = identityAtt ? parseIdentity(identityAtt.data) : null;
  const lotsRaw = decodeIf(lotsR, PROGRAMS.gate, "HolderLots");

  const tokenAccounts = (await getTokenHoldings(wallet))
    .filter((h) => h.mint === mint)
    .map((h) => ({ account: h.account, amount: h.amount, frozen: h.frozen }));

  let decision: Decision;
  if (!market) {
    decision = { reject: null, error: "mint has no credit-gate market", distinctInvestors: null, logs: [], payer: null };
  } else {
    // Any funded account can pay a simulation: the wallet itself, else the
    // market's authority. Nothing is signed or sent.
    const authority = market.config.gate_config_authority as Address;
    const [authR] = await getAccounts([authority]);
    const payer =
      walletR && walletR.lamports > 5_000_000n ? wallet : authR && authR.lamports > 5_000_000n ? authority : null;
    if (!payer) {
      decision = { reject: null, error: "no funded account to pay the simulation", distinctInvestors: null, logs: [], payer: null };
    } else {
      const ix = await validateIx(mint, wallet, payer, identityA, accreditationA, identity?.investorId ?? new Uint8Array(16));
      decision = await simulateValidate(ix, payer);
    }
  }

  return {
    mint,
    wallet,
    market,
    blocked: decodeIf(blockedR, PROGRAMS.gate, "BlockEntry"),
    identity: { address: identityA, att: identityAtt, parsed: identity },
    accreditation: {
      address: accreditationA,
      att: accreditationAtt,
      parsed: accreditationAtt ? parseAccreditation(accreditationAtt.data) : null,
    },
    link: decodeIf(linkR, PROGRAMS.gate, "HolderLink"),
    lots: lotsRaw ? lotsView(lotsRaw) : null,
    tokenAccounts,
    decision,
  };
}

/** What each `GateReject` variant means, in the gate's own terms (CG-P1). */
export const REJECT_TEXT: Record<string, string> = {
  Blocklisted: "The wallet is on the mint's block-list.",
  AttestationInvalid:
    "No valid SAS attestation: missing, not at the canonical address, wrong credential or schema, or not signed by a listed attester.",
  AttestationExpiry: "An attestation expires before the market's settlement horizon.",
  JurisdictionRejected: "The identity attestation's jurisdiction is not in the market policy.",
  GovernorCap: "Admitting another distinct investor would exceed the holder cap.",
  ConfigInvalid: "The market configuration cannot be used.",
  LotLedgerFull: "The holder's lot ledger has no free slot.",
  NotAccredited: "The market requires accreditation and the attestation says not accredited.",
  KycLevelTooLow: "The identity attestation's KYC level is below the market minimum.",
  InvestorIdChanged: "The wallet is linked to a different investor id than the one now attested.",
};

export { hex };
