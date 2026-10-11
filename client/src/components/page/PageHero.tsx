import type { ReactNode } from "react";

/** Top of a marketing sub-page: eyebrow, Cormorant title, lede. Clears the fixed header. */
export function PageHero({ eyebrow, title, children }: { eyebrow: string; title: ReactNode; children?: ReactNode }) {
  return (
    <section className="stipple-overlay relative border-b border-gold-500/15 bg-obsidian-950 px-6 pt-36 pb-16">
      <div className="mx-auto max-w-4xl text-center">
        <span className="mb-3 block font-cinzel text-xs font-semibold tracking-widest3 text-gold-400 uppercase">{eyebrow}</span>
        <h1 className="font-cormorant text-4xl leading-tight font-normal text-gold-100 sm:text-5xl lg:text-6xl">{title}</h1>
        {children && <p className="mx-auto mt-6 max-w-2xl text-sm leading-relaxed font-light text-gold-400 sm:text-base">{children}</p>}
      </div>
    </section>
  );
}

/** A titled band of content on a sub-page. */
export function Band({ title, eyebrow, children }: { title: string; eyebrow?: string; children: ReactNode }) {
  return (
    <section className="border-b border-gold-500/10 px-6 py-16">
      <div className="mx-auto max-w-5xl">
        {eyebrow && <span className="mb-2 block font-cinzel text-[11px] tracking-widest3 text-gold-600 uppercase">{eyebrow}</span>}
        <h2 className="mb-8 font-cormorant text-3xl text-gold-100 sm:text-4xl">{title}</h2>
        {children}
      </div>
    </section>
  );
}

/** A grid of etched cards. */
export function Cards({ items }: { items: { title: string; body: ReactNode; meta?: string }[] }) {
  return (
    <div className="grid grid-cols-1 gap-5 md:grid-cols-2 lg:grid-cols-3">
      {items.map((c) => (
        <div key={c.title} className="etched-card rounded-sm p-6">
          {c.meta && <p className="mb-2 font-mono text-[10px] tracking-widest2 text-gold-600 uppercase">{c.meta}</p>}
          <h3 className="mb-2 font-cinzel text-sm font-bold tracking-wider text-gold-100 uppercase">{c.title}</h3>
          <div className="text-[13px] leading-relaxed font-light text-gold-400">{c.body}</div>
        </div>
      ))}
    </div>
  );
}

/** Body copy with readable measure. */
export function Prose({ children }: { children: ReactNode }) {
  return (
    <div className="max-w-3xl space-y-4 text-sm leading-relaxed font-light text-gold-300 sm:text-[15px] [&_a]:text-gold-200 [&_a]:underline [&_a]:underline-offset-4 [&_li]:ml-5 [&_li]:list-disc [&_strong]:font-semibold [&_strong]:text-gold-100">
      {children}
    </div>
  );
}
