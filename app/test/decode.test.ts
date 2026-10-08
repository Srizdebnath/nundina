/**
 * The decoders against real program bytes: devnet accounts of the live
 * NPCS gate market (`fixtures/devnet-accounts.json`, refreshed with
 * `npx tsx test/fetch-fixtures.mts`) and a `ValidateDecision` event the
 * devnet gate emitted in simulation.
 */
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { fromBase64, hex } from "@/lib/borsh";
import { describeError, lotsView } from "@/lib/gate";
import { codecs, NPCS_MINT, pdas } from "@/lib/programs";
import { parseAccreditation, parseAttestation, parseIdentity } from "@/lib/sas";
import type { Address } from "@solana/kit";

const fx = JSON.parse(readFileSync(new URL("./fixtures/devnet-accounts.json", import.meta.url), "utf8")) as {
  mint: string;
  holder: string;
  accounts: Record<string, { address: string; owner: string; data: string }>;
  validateRejectLogs: string[];
};
const bytes = (k: string) => fromBase64(fx.accounts[k]!.data);

describe("credit-gate accounts (devnet NPCS market)", () => {
  it("derives the same PDAs the gate uses", async () => {
    expect(fx.mint).toBe(NPCS_MINT);
    expect(await pdas.gateConfig(NPCS_MINT)).toBe(fx.accounts.marketConfig!.address);
    expect(await pdas.gatePolicy(NPCS_MINT)).toBe(fx.accounts.marketPolicy!.address);
    expect(await pdas.lots(NPCS_MINT, fx.holder as Address)).toBe(fx.accounts.holderLotsH!.address);
  });

  it("decodes MarketConfig and MarketPolicy", () => {
    const c = codecs.gate.decodeAccount("MarketConfig", bytes("marketConfig"));
    expect(c.mint).toBe(NPCS_MINT);
    expect(c.seasoning_seconds).toBe(604_800n);
    expect(c.attester_count).toBe(1);
    expect((c.attesters as string[])[0]).toBe("2L8VPRJme9bEtqoBRTG3kR9JkfbT3GF1DBf48vp2ksQC");
    const p = codecs.gate.decodeAccount("MarketPolicy", bytes("marketPolicy"));
    expect(p.credential).toBe("7H1P4RLB9VhsrSHwqtxJJsMC5wMtF9T7HUN6EUovhfxV");
    expect(p.distinct_investors).toBeGreaterThanOrEqual(1);
    const j = (p.jurisdictions as Uint8Array[]).slice(0, p.jurisdiction_count as number).map((x) => String.fromCharCode(...x));
    expect(j).toContain("US");
  });

  it("decodes holder H's lot ledger: one 1-NPCS primary lot seasoning for 7 days", () => {
    const l = lotsView(codecs.gate.decodeAccount("HolderLots", bytes("holderLotsH")));
    expect(l.lots).toHaveLength(1);
    const [lot] = l.lots;
    expect(lot!.qty).toBe(1_000_000n);
    expect(lot!.source).toBe("PrimaryMint");
    expect(lot!.restrictionExpiry - lot!.acquiredTs).toBe(604_800);
  });

  it("rejects bytes of the wrong account type", () => {
    expect(() => codecs.gate.decodeAccount("HolderLots", bytes("marketConfig"))).toThrow(/not a HolderLots/);
  });

  it("parses H's SAS attestations the way the gate does", () => {
    const id = parseAttestation(bytes("identityH"))!;
    expect(id.nonce).toBe(fx.holder);
    expect(id.signer).toBe("2L8VPRJme9bEtqoBRTG3kR9JkfbT3GF1DBf48vp2ksQC");
    const ident = parseIdentity(id.data)!;
    expect(ident.jurisdiction).toBe("US");
    // The holder link stores the same investor id the attestation carries.
    const link = codecs.gate.decodeAccount("HolderLink", bytes("holderLinkH"));
    expect(hex(link.investor_id as Uint8Array)).toBe(hex(ident.investorId));
    const acc = parseAttestation(bytes("accreditationH"))!;
    expect(parseAccreditation(acc.data)).toEqual({ accredited: true, method: expect.any(Number) });
  });
});

describe("ValidateDecision", () => {
  it("decodes the program's typed rejection", () => {
    // Logs of simulating `validate` on devnet for a wallet with no attestations.
    const [ev] = codecs.gate.eventsFromLogs(fx.validateRejectLogs);
    expect(ev!.name).toBe("ValidateDecision");
    expect(ev!.data.reject).toBe("AttestationInvalid");
    expect(ev!.data.mint).toBe(NPCS_MINT);
    expect(ev!.data.distinct_investors).toBe(1);
  });

  it("names custom program errors from the IDL", () => {
    expect(describeError({ InstructionError: [0, { Custom: 6030 }] })).toBe("InvestorRecordMismatch");
    expect(describeError({ InstructionError: [0, { Custom: 6013n }] })).toBe("Blocklisted");
  });
});

describe("IDLs", () => {
  // CI's app job does not run `anchor build`; locally (and in the rails job)
  // the built IDLs must match what the app decodes with.
  const built = new URL("../../target/idl/", import.meta.url);
  for (const name of ["auction_engine", "credit_gate", "mark_engine", "queue_claim", "liquidity_facility", "tranche_engine", "npcs_treasury"]) {
    it.skipIf(!existsSync(new URL(`${name}.json`, built)))(`${name} matches anchor build (npm run sync-idl)`, () => {
      const ours = readFileSync(new URL(`../idl/${name}.json`, import.meta.url), "utf8");
      expect(JSON.parse(ours)).toEqual(JSON.parse(readFileSync(new URL(`${name}.json`, built), "utf8")));
    });
  }
  it("payment_rails matches the crank's IDL (kept in sync by the rails job's drift check)", () => {
    const ours = readFileSync(new URL("../idl/payment_rails.json", import.meta.url), "utf8");
    const crank = readFileSync(new URL("../../payment-rails/crank/idl/payment_rails.json", import.meta.url), "utf8");
    expect(JSON.parse(ours)).toEqual(JSON.parse(crank));
  });
});
