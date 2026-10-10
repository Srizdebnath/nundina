import { readFileSync } from "node:fs";
import path from "node:path";
import { getChapter } from "./docs";

/** Server-only: read a chapter's markdown source from disk. */
export const DOCS_DIR = path.join(process.cwd(), "src", "content", "docs");

export function readChapterSource(slug: string): string {
  const c = getChapter(slug);
  if (!c) return "";
  try {
    return readFileSync(path.join(DOCS_DIR, `${c.slug}.md`), "utf8");
  } catch {
    return `# Chapter ${c.n} — ${c.title}\n\n_Content forthcoming._\n`;
  }
}
