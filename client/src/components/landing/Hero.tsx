import Image from "next/image";
import type { ComponentType, SVGProps } from "react";
import { Button } from "@/components/ui/Button";
import { CompassStarIcon, LaurelIcon, TempleIcon } from "@/components/ui/Icons";
import { LogoMark } from "@/components/ui/Logo";
import { APP_URL } from "@/lib/site";
import heroBg from "@/assets/screen.png";

/* Darkens only the centre of the painting so the copy stays legible; edges keep full brightness. */
const COPY_HALO =
  "radial-gradient(ellipse 46% 42% at 50% 47%, rgba(5,5,7,0.68) 0%, rgba(5,5,7,0.45) 50%, transparent 100%)";

const PILLARS: { title: string; body: string; Icon: ComponentType<SVGProps<SVGSVGElement>> }[] = [
  { title: "NAV-Priced Credit", body: "Sealed-bid price discovery, not a passive AMM", Icon: LaurelIcon },
  { title: "On-Chain Transparency", body: "Token-2022 · Token ACL · SAS on Solana devnet", Icon: TempleIcon },
  { title: "No Phantom Yield", body: "One real pilot asset · NAV 1.000000", Icon: CompassStarIcon },
];

export function Hero() {
  return (
    <section
      id="hero"
      style={{ ["--hero-text-shadow" as string]: "0 1px 2px rgba(0,0,0,0.85), 0 2px 14px rgba(0,0,0,0.6)" }}
      className="relative flex min-h-[960px] flex-col justify-between overflow-hidden bg-obsidian-950 pt-24 pb-12 lg:min-h-[1020px]"
    >
      {/* Painted Roman landscape backdrop */}
      <div className="absolute inset-0 z-0 overflow-hidden">
        <Image
          src={heroBg}
          alt=""
          fill
          priority
          sizes="100vw"
          placeholder="blur"
          className="pointer-events-none object-cover object-center"
        />
        {/* Soft dark halo behind the copy only — painting stays at full brightness at the edges */}
        <div className="pointer-events-none absolute inset-0" style={{ background: COPY_HALO }} />
        {/* Short fade into the obsidian telemetry strip below */}
        <div className="pointer-events-none absolute inset-x-0 bottom-0 h-40 bg-gradient-to-t from-obsidian-950 to-transparent" />
      </div>

      {/* Copy framed inside the arch */}
      <div className="relative z-10 mx-auto flex max-w-5xl flex-1 flex-col items-center justify-center px-6 pt-16 text-center md:pt-24 lg:pt-28">
        <div className="mb-6 inline-flex h-12 w-12 items-center justify-center rounded-full border border-gold-400/40 bg-obsidian-900/80 shadow-[0_0_20px_rgba(234,193,131,0.2)] backdrop-blur-md">
          <LogoMark className="h-7 w-7" />
        </div>

        <div className="mb-6 flex items-center justify-center space-x-3">
          <span className="hidden h-px w-10 bg-gold-200/80 sm:inline-block" />
          <span className="[text-shadow:var(--hero-text-shadow)] font-cinzel text-xs font-semibold tracking-widest3 text-gold-200 uppercase md:text-sm">
            On-Chain Private Credit
          </span>
          <span className="hidden h-px w-10 bg-gold-200/80 sm:inline-block" />
        </div>

        <h1 className="[text-shadow:var(--hero-text-shadow)] mb-6 max-w-4xl font-cormorant text-5xl leading-[0.96] font-normal tracking-tight text-gold-100 drop-shadow-lg sm:text-6xl md:text-7xl lg:text-8xl">
          PRIVATE CREDIT,
          <br />
          <span className="font-light text-gold-300 italic">REIMAGINED.</span>
        </h1>

        <p className="mx-auto mb-10 max-w-xl text-base leading-relaxed font-normal text-gold-200 [text-shadow:var(--hero-text-shadow)] sm:text-lg md:text-xl">
          Gated credit, priced on-chain.
          <br className="hidden sm:inline" /> Built on Solana for a new financial era.
        </p>

        <div className="flex w-full max-w-md flex-col items-center justify-center space-y-4 sm:flex-row sm:space-y-0 sm:space-x-6">
          <Button href={APP_URL} arrow>
            Explore Nundina
          </Button>
          <Button href="/protocol" variant="outline" className="bg-obsidian-950/55">
            Read the Protocol
          </Button>
        </div>
      </div>

      {/* Three-pillar banner */}
      <div className="relative z-10 mt-16 w-full pt-6">
        <div className="mx-auto max-w-7xl px-6">
          <div className="hairline-gold mb-8 h-px w-full" />
          <div className="grid grid-cols-1 gap-6 text-gold-300 md:grid-cols-3">
            {PILLARS.map(({ title, body, Icon }) => (
              <div
                key={title}
                className="group flex items-center space-x-4 rounded-sm border border-gold-400/10 bg-obsidian-950/60 p-3 backdrop-blur-md"
              >
                <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full border border-gold-400/30 bg-gold-100/5 text-gold-300 shadow-[0_0_10px_rgba(234,193,131,0.1)] transition-colors group-hover:border-gold-200 group-hover:text-gold-100">
                  <Icon className="h-5 w-5" />
                </div>
                <div>
                  <span className="block font-cinzel text-xs font-bold tracking-widest2 text-gold-100 uppercase transition-colors group-hover:text-gold-200">
                    {title}
                  </span>
                  <p className="text-[11px] tracking-wider text-gold-500">{body}</p>
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
