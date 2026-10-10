"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { CHAPTERS, PARTS } from "@/lib/docs";

/** Sticky chapter navigation for the /docs section. */
export function DocsSidebar() {
  const pathname = usePathname();

  return (
    <nav aria-label="Documentation" className="hidden lg:block">
      <div className="sticky top-28 max-h-[calc(100vh-8rem)] overflow-y-auto pb-10 pr-2">
        <Link
          href="/docs"
          className={`font-mono text-[0.68rem] uppercase tracking-widest3 transition-colors ${
            pathname === "/docs" ? "text-gold-200" : "text-gold-500 hover:text-gold-200"
          }`}
        >
          Documentation
        </Link>
        {PARTS.map((part) => (
          <div key={part.id} className="mt-8">
            <p className="font-mono text-[0.6rem] uppercase tracking-widest3 text-gold-700">
              Part {part.id} · {part.label}
            </p>
            <ul className="mt-3 space-y-0.5 border-l border-gold-900/40">
              {CHAPTERS.filter((c) => c.part === part.id).map((c) => {
                const active = pathname === `/docs/${c.slug}`;
                return (
                  <li key={c.slug}>
                    <Link
                      href={`/docs/${c.slug}`}
                      className={`-ml-px block border-l py-1 pl-4 text-[0.82rem] transition-colors ${
                        active
                          ? "border-gold-400 text-gold-100"
                          : "border-transparent text-gold-500 hover:border-gold-600 hover:text-gold-200"
                      }`}
                    >
                      <span className="font-mono text-[0.68rem] text-gold-600">
                        {String(c.n).padStart(2, "0")}
                      </span>{" "}
                      {c.short}
                    </Link>
                  </li>
                );
              })}
            </ul>
          </div>
        ))}
      </div>
    </nav>
  );
}
