import { TELEMETRY } from "@/lib/data";
import { LiveSlot } from "./LiveSlot";

export function TelemetryStrip() {
  return (
    <section id="metrics" className="relative z-20 border-y border-gold-500/20 bg-obsidian-950 py-12">
      <div className="mx-auto max-w-7xl px-6">
        <div className="mb-8 flex flex-col gap-3 border-b border-gold-500/15 pb-4 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center space-x-3">
            <span className="relative flex h-2.5 w-2.5">
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-gold-400 opacity-75" />
              <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-gold-200" />
            </span>
            <span className="font-cinzel text-xs font-semibold tracking-widest2 text-gold-300 uppercase">
              Nundina Devnet Telemetry
            </span>
          </div>
          <span className="font-mono text-xs text-gold-400">
            <LiveSlot />
          </span>
        </div>

        <div className="grid grid-cols-2 gap-6 md:grid-cols-3 lg:grid-cols-5">
          {TELEMETRY.map((m, i) => (
            <div
              key={m.label}
              className={`etched-card group relative rounded-sm p-5 ${
                i === TELEMETRY.length - 1 ? "col-span-2 md:col-span-1" : ""
              }`}
            >
              <div className="mb-1 font-mono text-[11px] tracking-widest text-gold-600 uppercase">{m.label}</div>
              <div
                className={`font-cormorant text-3xl font-semibold lg:text-4xl ${
                  m.emphasis
                    ? "gold-gradient-text"
                    : "text-gold-100 transition-colors group-hover:text-gold-300"
                }`}
              >
                {m.value}
              </div>
              <div className="mt-2 flex items-center font-mono text-[11px] text-gold-400">
                {m.dot && <span className="mr-1.5 inline-block h-1.5 w-1.5 rounded-full bg-gold-300" />}
                <span>{m.note}</span>
              </div>
            </div>
          ))}
        </div>

        <p className="mt-6 font-mono text-[10px] tracking-wider text-gold-600 uppercase">
          Source: DEPLOYMENTS.md / STATUS.md · Solana devnet only · no mainnet deployment
        </p>
      </div>
    </section>
  );
}
