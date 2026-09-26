/**
 * Reconciliation: prove from chain state alone that escrowed balances equal
 * open obligations (R4). Every check is `expected` vs `actual`; a report is
 * clean only if all pass. Donated dust (a vault holding *more* than its
 * escrow records) is surfaced as a warning — it is the owner's on refund.
 */
import type { Address } from "@solana/kit";
import { fetchDecoded, listByKey, tokenBalance, type R } from "./chain.ts";
import { listEscrows, loadSession } from "./crank.ts";
import type { Struct } from "./idl.ts";

export interface Check {
  name: string;
  expected: bigint;
  actual: bigint;
  ok: boolean;
}

export interface Report {
  subject: string;
  ok: boolean;
  checks: Check[];
  warnings: string[];
}

const eq = (name: string, expected: bigint, actual: bigint): Check => ({ name, expected, actual, ok: expected === actual });

export async function reconcileSession(rpc: R, session: Address): Promise<Report> {
  const s = await loadSession(rpc, session);
  const escrows = await listEscrows(rpc, session);
  const receipts = await listByKey<Struct>(rpc, "FillReceipt", 8, session);
  const checks: Check[] = [];
  const warnings: string[] = [];

  const sum = (f: (e: (typeof escrows)[number]["data"]) => bigint) => escrows.reduce((a, x) => a + f(x.data), 0n);
  checks.push(eq("open escrows = session.open_escrows", BigInt(s.open_escrows), BigInt(escrows.length)));
  checks.push(eq("Σ escrow.cash = session.total_cash", s.total_cash, sum((e) => e.cash)));
  checks.push(eq("Σ escrow.bond = session.total_bonds", s.total_bonds, sum((e) => e.bond)));
  checks.push(eq("Σ escrow.asset = session.total_asset", s.total_asset, sum((e) => e.asset)));

  for (const { address, data: e } of escrows) {
    const usdc = (await tokenBalance(rpc, e.vault)) ?? -1n;
    const owed = e.cash + e.bond;
    checks.push({ name: `vault ${address.slice(0, 8)}… ≥ cash + bond`, expected: owed, actual: usdc, ok: usdc >= owed });
    if (usdc > owed) warnings.push(`vault ${e.vault} holds ${usdc - owed} donated micro-USDC (goes to owner on refund)`);
    if (e.side === "Sell") {
      const asset = (await tokenBalance(rpc, e.asset_vault)) ?? -1n;
      checks.push({ name: `asset vault ${address.slice(0, 8)}… ≥ asset`, expected: e.asset, actual: asset, ok: asset >= e.asset });
      if (asset > e.asset) warnings.push(`asset vault ${e.asset_vault} holds ${asset - e.asset} donated units`);
    }
  }

  const rsum = (f: (r: Struct) => bigint) => receipts.reduce((a, r) => a + f(r.data), 0n);
  checks.push(eq("receipts = session.fills_settled", BigInt(s.fills_settled), BigInt(receipts.length)));
  checks.push(eq("Σ receipt.qty = session.volume_settled", s.volume_settled, rsum((r) => r.qty as bigint)));
  checks.push(eq("Σ receipt.gross = session.gross_settled", s.gross_settled, rsum((r) => (r.split as Struct).gross as bigint)));
  for (const { address, data: r } of receipts) {
    const sp = r.split as Struct;
    const parts = (sp.to_seller as bigint) + (sp.fee as bigint) + (sp.to_reserve as bigint);
    checks.push(eq(`receipt ${address.slice(0, 8)}… seller+fee+reserve = gross`, sp.gross as bigint, parts));
  }

  return { subject: `session ${session}`, ok: checks.every((c) => c.ok), checks, warnings };
}

/** vault = funded − principal outstanding + interest collected. */
export async function reconcileFacility(rpc: R, facility: Address): Promise<Report> {
  const f = await fetchDecoded<Struct>(rpc, "Facility", facility);
  if (!f) throw new Error(`facility ${facility} not found`);
  const advances = await listByKey<Struct>(rpc, "Advance", 8, facility);
  const vault = (await tokenBalance(rpc, f.vault as Address)) ?? -1n;
  const expected = (f.total_funded as bigint) - (f.principal_outstanding as bigint) + (f.interest_collected as bigint);
  const checks: Check[] = [
    { name: "vault ≥ funded − outstanding + interest", expected, actual: vault, ok: vault >= expected },
    eq(
      "Σ advance.principal = facility.principal_outstanding",
      f.principal_outstanding as bigint,
      advances.reduce((a, x) => a + (x.data.principal as bigint), 0n),
    ),
  ];
  const warnings = vault > expected ? [`facility vault holds ${vault - expected} unaccounted micro-USDC (donation)`] : [];
  return { subject: `facility ${facility}`, ok: checks.every((c) => c.ok), checks, warnings };
}

export function formatReport(r: Report): string {
  const lines = [`${r.ok ? "✅" : "❌"} ${r.subject}`];
  for (const c of r.checks) lines.push(`  ${c.ok ? "ok " : "BAD"} ${c.name}: expected ${c.expected}, actual ${c.actual}`);
  for (const w of r.warnings) lines.push(`  ⚠ ${w}`);
  return lines.join("\n");
}
