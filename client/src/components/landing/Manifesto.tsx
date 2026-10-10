import { ECOSYSTEM } from "@/lib/data";

export function Manifesto() {
  return (
    <section className="relative overflow-hidden border-t border-gold-500/10 bg-obsidian-900 py-24 text-center">
      <div className="relative z-10 mx-auto max-w-4xl px-6">
        <div className="mb-2 font-cormorant text-6xl text-gold-400/40 select-none">“</div>
        <blockquote className="mb-6 font-cormorant text-3xl leading-relaxed font-light text-gold-100 italic sm:text-4xl lg:text-5xl">
          Not a venue. Not an AMM. The mechanism that prices gated credit.
        </blockquote>
        <div className="font-cinzel text-xs tracking-widest2 text-gold-400 uppercase">
          Nundina Design Principle
        </div>

        <div className="mt-16 border-t border-gold-500/15 pt-12">
          <div className="mb-6 font-mono text-[11px] tracking-widest text-gold-500 uppercase">
            Built On
          </div>
          <div className="flex flex-wrap items-center justify-center gap-x-10 gap-y-6 opacity-75 md:gap-x-12">
            {ECOSYSTEM.map((e) => (
              <span
                key={e.label}
                className="font-cinzel text-sm tracking-widest text-gold-300 uppercase transition-colors hover:text-gold-100"
              >
                {e.label}
                {e.planned && <span className="ml-1.5 font-mono text-[10px] text-gold-600 normal-case">(planned)</span>}
              </span>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
