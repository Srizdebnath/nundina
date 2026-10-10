import type { Metadata } from "next";
import Link from "next/link";
import { notFound } from "next/navigation";
import { DocsToc } from "@/components/docs/DocsToc";
import { Markdown } from "@/components/docs/Markdown";
import { CHAPTERS, extractHeadings, getChapter, neighbours } from "@/lib/docs";
import { readChapterSource } from "@/lib/docs-server";

export function generateStaticParams() {
  return CHAPTERS.map((c) => ({ slug: c.slug }));
}

export async function generateMetadata({
  params,
}: {
  params: Promise<{ slug: string }>;
}): Promise<Metadata> {
  const { slug } = await params;
  const chapter = getChapter(slug);
  if (!chapter) return { title: "Docs — NUNDINA" };
  return { title: `${chapter.title} — NUNDINA Docs`, description: chapter.blurb };
}

export default async function ChapterPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const chapter = getChapter(slug);
  if (!chapter) notFound();

  const source = readChapterSource(slug);
  const headings = extractHeadings(source);
  const { prev, next } = neighbours(slug);

  return (
    <div className="py-4 xl:grid xl:grid-cols-[minmax(0,1fr)_200px] xl:gap-12">
      <article className="min-w-0 max-w-3xl">
        <p className="font-mono text-[0.68rem] uppercase tracking-widest3 text-gold-600">
          Chapter {String(chapter.n).padStart(2, "0")} · Part {chapter.part}
        </p>
        <Markdown source={source} headings={headings} />

        <nav className="mt-16 flex items-center justify-between gap-6 border-t border-gold-900/40 pt-6 text-sm">
          {prev ? (
            <Link href={`/docs/${prev.slug}`} className="text-gold-500 transition-colors hover:text-gold-200">
              ← {prev.title}
            </Link>
          ) : (
            <span />
          )}
          {next ? (
            <Link
              href={`/docs/${next.slug}`}
              className="text-right text-gold-500 transition-colors hover:text-gold-200"
            >
              {next.title} →
            </Link>
          ) : (
            <span />
          )}
        </nav>
      </article>
      <DocsToc headings={headings} />
    </div>
  );
}
