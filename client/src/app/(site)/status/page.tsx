import type { Metadata } from "next";
import Link from "next/link";
import { Suspense } from "react";
import { connection } from "next/server";
import { Band, Cards, PageHero, Prose } from "@/components/page/PageHero";
import { CLUSTER } from "@/lib/programs";
import { deployedMap, REGISTRY } from "@/lib/registry";

export const metadata: Metadata = {
  title: "Roadmap & Status",
  description: "Where the NUNDINA pilot stands: built, deployed, and what remains before mainnet.",
};

async function DeployedCount() {
  await connection();
  const d = await deployedMap();
  if (!d) return <>—</>;
  return <>{REGISTRY.filter((r) => d[r.key]).length}/{REGISTRY.length}</>;
}

const PHASES = [
  { meta: "done", title: "Programs built", body: "All eight Anchor programs have code, tests and IDLs, sharing the credit-math crate." },
  { meta: "done", title: "Gate live on devnet", body: "Default-frozen NPCS mint, Token ACL → credit-gate thaw, SAS attestations, holder admission." },
  { meta: "in progress", title: "Live devnet session", body: "A recorded sealed-bid auction with DvP settlement over NPCS, then a live queue-claim sale." },
  { meta: "in progress", title: "Facility & treasury", body: "Deploy liquidity-facility after the credit-gate custody upgrade; hand the NPCS mint to the treasury." },
  { meta: "next", title: "External audit", body: "Programs frozen, then an external audit with criticals and highs fixed and re-reviewed." },
  { meta: "gated", title: "Mainnet pilot", body: "All twelve acceptance scenarios re-run on mainnet-beta with real USDC, after counsel clears the offering gate." },
];

export default function StatusPage() {
  return (
    <>
      <PageHero eyebrow="Roadmap & Status" title="Where the Pilot Stands">
        Mainnet preconditions are gated, not scheduled. This page states what is real today and what is not.
      </PageHero>
      <Band eyebrow="Today" title="By the numbers">
        <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
          {[
            ["Programs built", "8/8"],
            [`Deployed on ${CLUSTER}`, <Suspense key="d" fallback="…"><DeployedCount /></Suspense>],
            ["On mainnet", "0/8"],
            ["External audit", "Not started"],
          ].map(([k, v], i) => (
            <div key={i} className="etched-card rounded-sm p-5 text-center">
              <div className="font-mono text-2xl text-gold-100">{v}</div>
              <div className="mt-2 font-cinzel text-[10px] tracking-widest2 text-gold-500 uppercase">{k}</div>
            </div>
          ))}
        </div>
      </Band>
      <Band eyebrow="Roadmap" title="Phases">
        <Cards items={PHASES} />
        <div className="mt-8">
          <Prose>
            <p>
              Live program status: <Link href="/deployments">Deployments</Link>. Full plan and mainnet preconditions:{" "}
              <Link href="/docs/15-legal-risk-audit-roadmap">Legal, Risk, Audit & Roadmap</Link>.
            </p>
          </Prose>
        </div>
      </Band>
    </>
  );
}
