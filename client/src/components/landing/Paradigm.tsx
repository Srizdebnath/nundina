import { SectionHeading } from "@/components/ui/SectionHeading";

const PILLARS = [
  {
    numeral: "I",
    title: "Tokenized Credit on Solana",
    body: "The pilot sleeve NPCS is a Token-2022 mint, frozen by default and thawed only through Token ACL once credit-gate validates the holder's SAS attestations. Not a Delaware SPV — that is roadmap.",
    footLabel: "Token Standard",
    footValue: "Token-2022 + sRFC-37",
  },
  {
    numeral: "II",
    title: "Three-Book Tranche Waterfall",
    body: "tranche-engine runs senior / junior pools with three-book accounting and a coupon waterfall. The program is deployed on devnet; no pool has been opened yet — we say so.",
    footLabel: "tranche-engine",
    footValue: "Deployed · No Pool Yet",
  },
  {
    numeral: "III",
    title: "On-Chain NAV, Published Weekly",
    body: "mark-engine publishes the NAV on-chain with an inputs hash, and moves feeds through Fresh → Warned → Stale → Suspended so a stale appraisal can never be traded as fresh.",
    footLabel: "Mark Cadence",
    footValue: "Weekly · mark-engine",
  },
];

export function Paradigm() {
  return (
    <section id="paradigm" className="relative overflow-hidden bg-obsidian-900 py-28">
      <div className="pointer-events-none absolute -top-40 left-1/2 h-[500px] w-[800px] -translate-x-1/2 rounded-full bg-gold-300/5 blur-3xl" />
      <div className="relative z-10 mx-auto max-w-7xl px-6">
        <SectionHeading eyebrow="The Architectural Paradigm" title="Institutional Credit on Verifiable Rails." className="mb-20 max-w-3xl" size="large">
          Tokenized private-credit funds are gated at ~5% a quarter and marked quarterly by their own
          managers. NUNDINA is the mechanism, not the venue — it never puts a passive LP in front of a
          stale NAV.
        </SectionHeading>

        <div className="grid grid-cols-1 gap-8 md:grid-cols-3">
          {PILLARS.map((p) => (
            <div
              key={p.numeral}
              className="etched-card group relative flex flex-col justify-between rounded-sm p-8 transition-all duration-300 hover:-translate-y-1"
            >
              <div>
                <div className="mb-6 flex h-12 w-12 items-center justify-center rounded-sm border border-gold-400/40 bg-obsidian-850 font-cinzel text-xl font-bold text-gold-200 shadow-[inset_0_0_8px_rgba(254,236,190,0.06)]">
                  {p.numeral}
                </div>
                <h3 className="mb-4 font-cormorant text-2xl text-gold-100 transition-colors group-hover:text-gold-300 sm:text-3xl">
                  {p.title}
                </h3>
                <p className="mb-6 text-sm leading-relaxed font-light text-gold-400">{p.body}</p>
              </div>
              <div className="flex items-center justify-between border-t border-gold-500/15 pt-4 font-mono text-xs text-gold-400">
                <span>{p.footLabel}</span>
                <span className="text-gold-200">{p.footValue}</span>
              </div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
