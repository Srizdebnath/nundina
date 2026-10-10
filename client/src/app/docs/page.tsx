import type { Metadata } from "next";
import Link from "next/link";
import { CHAPTERS, PARTS } from "@/lib/docs";

export const metadata: Metadata = {
  title: "Docs — NUNDINA",
  description:
    "The NUNDINA handbook: fifteen chapters on the pricing, queue & risk-transfer engine for gated, NAV-priced private credit on Solana. Devnet pilot.",
};

export default function DocsIndexPage() {
  return (
    <div className="max-w-3xl py-4">
      <p className="font-mono text-[0.7rem] uppercase tracking-widest3 text-gold-600">Documentation</p>
      <h1 className="mt-3 font-cinzel text-4xl text-gold-200">The NUNDINA Handbook</h1>
      <p className="mt-5 leading-relaxed text-gold-300/90">
        Fifteen chapters covering the problem, the architecture, every on-chain program, the
        off-chain services, the NPCS pilot asset, and the legal &amp; audit posture — in reading
        order. Every claim is sourced to the repository or an external reference.
      </p>
      <div className="mt-6 rounded-lg border border-gold-700/30 bg-obsidian-850/50 px-5 py-4 text-sm text-gold-400">
        <span className="font-mono text-[0.68rem] uppercase tracking-widest2 text-gold-500">
          Pilot notice
        </span>
        <p className="mt-2">
          NUNDINA is a <strong className="text-gold-300">devnet pilot</strong>. 7 of 8 programs are
          deployed to devnet, 0 to mainnet. It is not audited and this is not an offer or
          investment advice.
        </p>
      </div>

      {PARTS.map((part) => (
        <section key={part.id} className="mt-12">
          <h2 className="font-mono text-[0.68rem] uppercase tracking-widest3 text-gold-600">
            Part {part.id} · {part.label}
          </h2>
          <ul className="mt-4 space-y-3">
            {CHAPTERS.filter((c) => c.part === part.id).map((c) => (
              <li key={c.slug}>
                <Link
                  href={`/docs/${c.slug}`}
                  className="etched-card group block rounded-lg px-5 py-4 transition-colors"
                >
                  <div className="flex items-baseline gap-3">
                    <span className="font-mono text-xs text-gold-600">
                      {String(c.n).padStart(2, "0")}
                    </span>
                    <span className="font-cinzel text-lg text-gold-200 group-hover:text-gold-100">
                      {c.title}
                    </span>
                  </div>
                  <p className="mt-1 pl-8 text-sm text-gold-500">{c.blurb}</p>
                </Link>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
