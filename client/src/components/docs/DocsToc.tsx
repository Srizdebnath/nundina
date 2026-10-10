import type { Heading } from "@/lib/docs";

/** Per-chapter "On this page" table of contents. */
export function DocsToc({ headings }: { headings: Heading[] }) {
  if (headings.length < 2) return null;
  return (
    <nav aria-label="On this page" className="hidden xl:block">
      <div className="sticky top-28">
        <p className="font-mono text-[0.6rem] uppercase tracking-widest3 text-gold-700">On this page</p>
        <ul className="mt-3 space-y-2 text-[0.8rem]">
          {headings.map((h) => (
            <li key={h.id} className={h.level === 3 ? "pl-4" : h.level >= 4 ? "pl-8" : ""}>
              <a href={`#${h.id}`} className="text-gold-500 transition-colors hover:text-gold-200">
                {h.text}
              </a>
            </li>
          ))}
        </ul>
      </div>
    </nav>
  );
}
