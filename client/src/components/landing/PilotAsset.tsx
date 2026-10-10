import { SmartLink } from "@/components/ui/SmartLink";
import { GATE_CRISIS, NPCS, shortAddress, type MarketCard } from "@/lib/data";
import { APP_URL, PILOT_DISCLAIMER } from "@/lib/site";

function Stat({ label, value, strong }: { label: string; value: string; strong?: boolean }) {
  return (
    <div>
      <div className="font-mono text-[10px] text-gold-600 uppercase">{label}</div>
      {strong ? (
        <div className="font-cormorant text-2xl font-bold text-gold-200">{value}</div>
      ) : (
        <div className="mt-1 text-sm font-semibold text-gold-300">{value}</div>
      )}
    </div>
  );
}

function Bar({ label, value, pct }: { label: string; value: string; pct: number }) {
  return (
    <div className="mb-6">
      <div className="mb-2 flex justify-between font-mono text-xs text-gold-500">
        <span>{label}</span>
        <span className="text-gold-300">{value}</span>
      </div>
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-obsidian-800">
        <div className="h-full rounded-full bg-gradient-to-r from-gold-600 to-gold-300" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

function MarketDataCard({ card }: { card: MarketCard }) {
  return (
    <div className="etched-card group relative flex flex-col justify-between rounded-sm p-6 transition-all sm:p-7">
      <div>
        <div className="mb-4 flex items-center justify-between">
          <span className="rounded-sm border border-gold-400/30 bg-gold-100/10 px-2.5 py-1 font-cinzel text-[10px] font-bold tracking-widest text-gold-200 uppercase">
            {card.tag}
          </span>
          <span className="rounded-sm border border-obsidian-600 bg-obsidian-800 px-2.5 py-1 font-mono text-[10px] text-gold-400 uppercase">
            {card.chip}
          </span>
        </div>
        <h3 className="mb-2 font-cormorant text-2xl font-bold text-gold-100 transition-colors group-hover:text-gold-300">
          {card.title}
        </h3>
        <p className="mb-6 text-xs font-light text-gold-500">{card.blurb}</p>
        <div className="mb-6 grid grid-cols-2 gap-4 border-y border-gold-500/15 py-4">
          {card.stats.map((s) => (
            <Stat key={s.label} {...s} />
          ))}
        </div>
        <Bar {...card.bar} />
      </div>
      <SmartLink
        href={card.source.href ?? "#"}
        className="block w-full rounded-sm border border-gold-400/40 bg-transparent py-3.5 text-center font-cinzel text-[11px] font-semibold tracking-widest text-gold-300 uppercase transition-all hover:border-gold-200 hover:bg-gold-100/10"
      >
        Source: {card.source.label} · {card.source.date} ↗
      </SmartLink>
    </div>
  );
}

function NpcsCard() {
  return (
    <div className="etched-card group relative flex flex-col justify-between rounded-sm border-gold-400/60 p-6 shadow-[0_0_35px_rgba(234,193,131,0.12)] transition-all sm:p-7">
      <div className="absolute -top-3 right-6 rounded-sm bg-gold-200 px-3 py-0.5 font-cinzel text-[10px] font-black tracking-widest text-obsidian-950 uppercase shadow-md">
        Devnet Pilot
      </div>
      <div>
        <div className="mb-4 flex items-center justify-between">
          <span className="rounded-sm border border-gold-300/50 bg-gold-200/20 px-2.5 py-1 font-cinzel text-[10px] font-bold tracking-widest text-gold-100 uppercase">
            Pilot Asset
          </span>
          <span className="rounded-sm border border-obsidian-600 bg-obsidian-800 px-2.5 py-1 font-mono text-[10px] text-gold-300 uppercase">
            {NPCS.symbol} · Token-2022
          </span>
        </div>
        <h3 className="mb-2 font-cormorant text-2xl font-bold text-gold-100 transition-colors group-hover:text-gold-300">
          {NPCS.name}
        </h3>
        <p className="mb-6 text-xs font-light text-gold-400">
          Frozen-by-default mint, gated through Token ACL → credit-gate, Squads-owned. Redemption via
          queue-claim; secondary trades via sealed-bid auction.
        </p>
        <div className="mb-6 grid grid-cols-2 gap-4 border-y border-gold-500/15 py-4">
          <div>
            <div className="font-mono text-[10px] text-gold-600 uppercase">NAV (Publication #1)</div>
            <div className="gold-gradient-text font-cormorant text-2xl font-bold">{NPCS.nav}</div>
          </div>
          <Stat label="Supply" value={`${NPCS.supply} ${NPCS.symbol}`} />
          <Stat label="Holder Cap / Seasoning" value={`${NPCS.holderCap} · ${NPCS.seasoningDays} days`} />
          <div>
            <div className="font-mono text-[10px] text-gold-600 uppercase">Mint (Devnet)</div>
            <SmartLink
              href={NPCS.explorer}
              className="mt-1 block font-mono text-sm font-semibold text-gold-200 underline decoration-gold-500/30 underline-offset-4 hover:decoration-gold-200"
            >
              {shortAddress(NPCS.mint)} ↗
            </SmartLink>
          </div>
        </div>
        <div className="mb-6 flex justify-between font-mono text-xs text-gold-400">
          <span>Yield · APY · Rating</span>
          <span className="text-gold-200">None claimed</span>
        </div>
      </div>
      <div>
        <SmartLink
          href={APP_URL}
          className="gold-btn-gradient block w-full rounded-sm py-3.5 text-center font-cinzel text-xs font-bold tracking-widest2 text-obsidian-950 uppercase transition-all hover:brightness-110"
        >
          Open in App →
        </SmartLink>
        <p className="mt-3 text-center font-mono text-[10px] tracking-wider text-gold-600">{PILOT_DISCLAIMER}</p>
      </div>
    </div>
  );
}

export function PilotAsset() {
  const [left, right] = GATE_CRISIS;
  return (
    <section id="pilot" className="relative border-t border-gold-500/20 bg-obsidian-950 py-28">
      <div className="mx-auto max-w-7xl px-6">
        <div className="mb-16 flex flex-col justify-between md:flex-row md:items-end">
          <div>
            <span className="mb-3 block font-cinzel text-xs font-semibold tracking-widest3 text-gold-400 uppercase">
              The 2026 Gate Crisis
            </span>
            <h2 className="font-cormorant text-4xl leading-tight font-normal text-gold-100 sm:text-5xl">
              Where Liquidity Is Gated
            </h2>
          </div>
          <p className="mt-4 max-w-md text-sm font-light text-gold-400 md:mt-0">
            Semi-liquid credit funds honor redemptions up to ~5% of NAV a quarter and pro-rate the rest.
            NUNDINA turns that denied request into a dated, priceable claim — proven on one real devnet
            asset. Side cards are independent market data, not NUNDINA figures.
          </p>
        </div>

        <div className="grid grid-cols-1 gap-8 lg:grid-cols-3">
          <MarketDataCard card={left} />
          <NpcsCard />
          <MarketDataCard card={right} />
        </div>
      </div>
    </section>
  );
}
