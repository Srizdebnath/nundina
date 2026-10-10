import type { Components } from "react-markdown";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { SmartLink } from "@/components/ui/SmartLink";
import type { Heading } from "@/lib/docs";
import { Mermaid } from "./Mermaid";

/**
 * Renders a chapter's markdown with the NUNDINA typographic system.
 * Heading ids are assigned in document order from `headings` (produced by
 * `extractHeadings` on the same source) so the on-page ToC links always resolve.
 *
 * Supported markdown subset: see `src/content/docs/AUTHORING.md`.
 */
export function Markdown({ source, headings }: { source: string; headings: Heading[] }) {
  let cursor = 0;

  const components: Components = {
    h1: ({ children }) => (
      <h1 className="mt-2 font-cinzel text-3xl leading-tight text-gold-200 sm:text-4xl">{children}</h1>
    ),
    h2: ({ children }) => {
      const id = headings[cursor++]?.id;
      return (
        <h2
          id={id}
          className="mt-14 scroll-mt-28 border-t border-gold-900/40 pt-8 font-cinzel text-2xl leading-tight text-gold-200"
        >
          {children}
        </h2>
      );
    },
    h3: ({ children }) => {
      const id = headings[cursor++]?.id;
      return (
        <h3 id={id} className="mt-10 scroll-mt-28 font-cinzel text-xl leading-snug text-gold-200">
          {children}
        </h3>
      );
    },
    h4: ({ children }) => {
      const id = headings[cursor++]?.id;
      return (
        <h4 id={id} className="mt-8 scroll-mt-28 font-mono text-sm font-bold uppercase tracking-widest2 text-gold-400">
          {children}
        </h4>
      );
    },
    p: ({ children }) => <p className="my-4 leading-relaxed text-gold-300/90">{children}</p>,
    strong: ({ children }) => <strong className="font-semibold text-gold-200">{children}</strong>,
    em: ({ children }) => <em className="italic text-gold-200">{children}</em>,
    hr: () => <hr className="my-10 border-gold-900/40" />,
    a: ({ href, children }) => (
      <SmartLink
        href={href ?? "#"}
        className="text-gold-200 underline decoration-gold-700/60 underline-offset-4 transition-colors hover:text-gold-100 hover:decoration-gold-400"
      >
        {children}
      </SmartLink>
    ),
    ul: ({ children }) => <ul className="my-5 list-disc space-y-2 pl-6 marker:text-gold-600">{children}</ul>,
    ol: ({ children }) => <ol className="my-5 list-decimal space-y-2 pl-6 marker:text-gold-600">{children}</ol>,
    li: ({ children }) => <li className="leading-relaxed text-gold-300/90">{children}</li>,
    blockquote: ({ children }) => (
      <blockquote className="my-6 border-l-2 border-gold-600/60 bg-obsidian-850/50 px-5 py-1 text-gold-400 italic">
        {children}
      </blockquote>
    ),
    table: ({ children }) => (
      <div className="my-6 overflow-x-auto rounded-lg border border-gold-900/40">
        <table className="w-full border-collapse text-sm">{children}</table>
      </div>
    ),
    thead: ({ children }) => <thead className="bg-obsidian-850/60 text-left">{children}</thead>,
    th: ({ children }) => (
      <th className="border-b border-gold-700/40 px-3 py-2 font-mono text-[0.68rem] uppercase tracking-widest2 text-gold-500">
        {children}
      </th>
    ),
    td: ({ children }) => (
      <td className="border-t border-gold-900/30 px-3 py-2 align-top text-gold-300/90">{children}</td>
    ),
    pre: ({ children }) => {
      const child = Array.isArray(children) ? children[0] : children;
      const className = (child as { props?: { className?: string } } | null)?.props?.className;
      if (typeof className === "string" && className.includes("language-mermaid")) {
        const chart = String(
          (child as { props?: { children?: unknown } } | null)?.props?.children ?? "",
        ).replace(/\n$/, "");
        return <Mermaid chart={chart} />;
      }
      return (
        <pre className="my-6 overflow-x-auto rounded-lg border border-gold-900/40 bg-obsidian-950 p-4 font-mono text-[0.82rem] leading-relaxed text-gold-300">
          {children}
        </pre>
      );
    },
    code: ({ className, children }) => {
      const isBlock = typeof className === "string" && className.startsWith("language-");
      if (isBlock) return <code className="font-mono">{children}</code>;
      return (
        <code className="rounded border border-gold-900/50 bg-obsidian-800 px-1.5 py-0.5 font-mono text-[0.85em] text-gold-200">
          {children}
        </code>
      );
    },
  };

  return (
    <div className="text-[0.95rem]">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {source}
      </ReactMarkdown>
    </div>
  );
}
