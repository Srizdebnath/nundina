"use client";

import { useEffect, useRef, useState } from "react";

/**
 * Client-only Mermaid renderer. The library is dynamically imported so it stays
 * out of the server bundle and the initial JS for non-diagram chapters.
 * Falls back to a readable <pre> if rendering fails.
 */
export function Mermaid({ chart }: { chart: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const mermaid = (await import("mermaid")).default;
        mermaid.initialize({
          startOnLoad: false,
          theme: "dark",
          securityLevel: "strict",
          fontFamily: "var(--ff-jakarta), system-ui, sans-serif",
          themeVariables: {
            background: "#050507",
            primaryColor: "#14161b",
            primaryTextColor: "#f7e2b5",
            primaryBorderColor: "#8a6f48",
            lineColor: "#a98d64",
            tertiaryColor: "#0e0f13",
          },
        });
        const id = `mermaid-${Math.random().toString(36).slice(2)}`;
        const { svg } = await mermaid.render(id, chart);
        if (!cancelled && ref.current) ref.current.innerHTML = svg;
      } catch {
        if (!cancelled) setFailed(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [chart]);

  if (failed) {
    return (
      <pre className="my-6 overflow-x-auto rounded-lg border border-gold-900/40 bg-obsidian-950 p-4 font-mono text-[0.82rem] leading-relaxed text-gold-300">
        {chart}
      </pre>
    );
  }

  return (
    <div
      ref={ref}
      aria-label="Diagram"
      className="my-6 flex justify-center overflow-x-auto rounded-lg border border-gold-900/40 bg-obsidian-950/60 p-6"
    />
  );
}
