# Agent Instructions — NUNDINA

Context for AI agents (Copilot coding agent, Codebuff, or humans pairing with either) working in this repository.

## What this repo is

NUNDINA is a private-credit secondary-market protocol on Solana: a pricing, queue and risk-transfer engine for gated, NAV-priced credit. Right now it is a **design-stage repository** — the corpus under `preresearch/` is the source of truth. Program code (Anchor/Rust) and the app (Next.js 16) arrive with the pilot build specified in `preresearch/architecture/08-SRS-Pilot-Rails.md`.

## Repo map

| Path | What it is | Status |
|---|---|---|
| `preresearch/architecture/00-README.md` | Doc index + design pillars | Current |
| `preresearch/architecture/05-Problem-Register.md` | ~100 failure modes (IDs A1…M8), ranked kill-list | Current |
| `preresearch/architecture/06-Solution-Architecture-v2.md` | Six-program design, §1–§15, traceability matrix | Current — **the architecture reference** |
| `preresearch/architecture/07-Legal-Topology-and-Compliance.md` | Legal register L1–L27, counsel gates Q1–Q20, program requirements CG-*/AE-*/QC-*/LF-*/TE-*/ME-* | Current |
| `preresearch/architecture/08-SRS-Pilot-Rails.md` | Pilot build spec: 12 acceptance scenarios (AC-1…AC-12), phase plan P0–P8 | Current — **the build contract** |
| `preresearch/research/12-Validation-2026-Reality-Check.md` | Dated market evidence + source-confidence labels | Current |
| `preresearch/architecture/01…04-*.md` | Superseded v1 docs | ⛔ Marked superseded — kept for history |

## Hard guardrails (non-negotiable)

1. **Never invent legal or regulatory conclusions.** Legal questions are tracked as counsel gates Q1–Q20 in doc `07`. If a change touches anything legal, reference the gate ID — do not resolve it in prose or code.
2. **Never fabricate numbers or citations.** Market facts carry confidence labels in doc `12`. New claims need a dated source with the same labeling discipline.
3. **Do not "modernize" the superseded docs (01–04).** They are historical records; they are marked superseded by `00-README.md` on purpose.
4. **Diagrams are Mermaid and must render on GitHub.** Preserve the existing diagram style (`flowchart`/`stateDiagram-v2`/`sequenceDiagram`, quoted labels, `<br/>` line breaks).
5. **Cross-links are relative** (e.g. `../research/12-…` from architecture docs). A CI job (lychee, offline mode) fails the build on broken internal links.
6. **ID discipline:** problem IDs (A1…M8), legal register (L1–L27), counsel gates (Q1–Q20), program requirements (CG-/AE-/QC-/LF-/TE-/ME-), acceptance criteria (AC-1…AC-12) are stable references used across docs. When adding entries, append new IDs — never renumber existing ones.
7. **Compliance requirements are load-bearing.** Changes to gate/auction/facility logic must trace to a requirement ID in doc `07` Part III and a problem ID in doc `05`. The traceability matrix in `06` §14 must stay consistent.
8. **No secrets, keys, or wallet material in commits.** `.gitignore` covers common patterns (`wallet.json`, `id.json`, `.env*`, `*.key`). If one slips in, stop and flag it immediately.

## Conventions

- Markdown tables over prose for structured data; keep the severity/likelihood conventions (S1–S4, H/M/L) from doc `05`.
- Confidence markers `[PRIMARY]` / `[SECONDARY]` / `[UNVERIFIED]` — use them on any new factual claim (see doc `07` appendix for definitions).
- Code comments and docs use US spelling; product docs use en-dashes and the existing voice (dry, adversarial, concrete).
- Commits: imperative subject ≤ 72 chars, blank line, motivation body.

## Working on the pilot (when `programs/` and `app/` exist)

- Anchor (Rust) programs under `programs/`, shared math in `credit-math` crate: u128 intermediates, **round against the initiator**.
- Next.js 16 app under `app/` (App Router, RSC-first, TypeScript strict, Tailwind + shadcn/ui).
- UI reads from the indexer (Postgres), never assembles write transactions server-side — the wallet signs everything client-side.
- Every acceptance scenario AC-1…AC-12 in doc `08` maps to an E2E test. Don't mark an AC done without its test passing on devnet staging.
