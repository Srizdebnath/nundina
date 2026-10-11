import type { Metadata } from "next";
import { Suspense } from "react";
import { connection } from "next/server";
import { Band, PageHero, Prose } from "@/components/page/PageHero";
import { CLUSTER, explorer, NPCS_MINT, PROGRAMS, SAS_PROGRAM } from "@/lib/programs";
import { SQUADS_VAULT } from "@/lib/admin";
import { deployedMap, REGISTRY } from "@/lib/registry";
import { REPO_URL } from "@/lib/site";

export const metadata: Metadata = {
  title: "Deployments",
  description: "Every NUNDINA program id and pilot account on Solana devnet, with live deployment status.",
};

function AddressCell({ a }: { a: string }) {
  return (
    <a href={explorer("address", a)} target="_blank" rel="noreferrer" className="font-mono text-[12px] break-all text-gold-200 underline-offset-4 hover:underline">
      {a}
    </a>
  );
}

function StatusCell({ state }: { state: boolean | null | undefined }) {
  if (state === undefined) return <span className="text-gold-600">checking…</span>;
  if (state === null) return <span className="text-gold-600">RPC unreachable</span>;
  return state ? (
    <span className="inline-flex items-center gap-2 text-emerald-300"><span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />deployed</span>
  ) : (
    <span className="text-amber-300">not deployed</span>
  );
}

function ProgramTable({ deployed }: { deployed?: Record<string, boolean> | null }) {
  return (
    <div className="overflow-x-auto rounded-sm border border-gold-500/15">
      <table className="w-full min-w-[640px] text-left text-sm">
        <thead className="border-b border-gold-500/20 bg-obsidian-850 font-cinzel text-[11px] tracking-widest2 text-gold-500 uppercase">
          <tr><th className="px-4 py-3">Program</th><th className="px-4 py-3">Program id</th><th className="px-4 py-3">Status</th></tr>
        </thead>
        <tbody>
          {REGISTRY.map((r) => (
            <tr key={r.key} className="border-t border-gold-500/10 align-top">
              <td className="px-4 py-3 font-mono text-[13px] text-gold-100">{r.name}</td>
              <td className="px-4 py-3"><AddressCell a={PROGRAMS[r.key]} /></td>
              <td className="px-4 py-3 text-[13px]"><StatusCell state={deployed === undefined ? undefined : deployed === null ? null : deployed[r.key]} /></td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

async function LivePrograms() {
  await connection();
  return <ProgramTable deployed={await deployedMap()} />;
}

export default function DeploymentsPage() {
  return (
    <>
      <PageHero eyebrow={`Solana ${CLUSTER}`} title="Deployments">
        Every program id below is configured from the environment and linked to the Solana Explorer. Status is read live from the cluster on each visit.
      </PageHero>
      <Band eyebrow="Programs" title="Program registry">
        <Suspense fallback={<ProgramTable />}>
          <LivePrograms />
        </Suspense>
      </Band>
      <Band eyebrow="Accounts" title="Pilot asset & governance">
        <div className="overflow-x-auto rounded-sm border border-gold-500/15">
          <table className="w-full min-w-[640px] text-left text-sm">
            <tbody>
              {[
                ["NPCS mint (Token-2022, default-frozen)", NPCS_MINT],
                ["Squads v4 vault (2-of-2, 48h timelock)", SQUADS_VAULT],
                ["Solana Attestation Service", SAS_PROGRAM],
              ].map(([k, v]) => (
                <tr key={k} className="border-t border-gold-500/10 first:border-t-0 align-top">
                  <td className="px-4 py-3 text-[13px] text-gold-300">{k}</td>
                  <td className="px-4 py-3"><AddressCell a={v} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="mt-8">
          <Prose>
            <p>
              Program ids stay the same across upgrades. Transaction signatures, build hashes and verification steps for every deployment are recorded in{" "}
              <a href={`${REPO_URL}/blob/main/DEPLOYMENTS.md`} target="_blank" rel="noreferrer">DEPLOYMENTS.md</a>. Nothing is deployed on mainnet.
            </p>
          </Prose>
        </div>
      </Band>
    </>
  );
}
