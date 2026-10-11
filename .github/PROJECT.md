# GitHub Project / board conventions

## Labels (create these in the repo)

| Label | Color | Use |
|---|---|---|
| `docs` | `0075ca` | Documentation changes |
| `architecture` | `1d76db` | Doc 05/06 changes |
| `legal` | `b60205` | Doc 07 changes — **never resolves a counsel gate in a PR** |
| `research` | `5319e7` | Doc 12 / new market evidence |
| `onchain` | `0e8a16` | `programs/**` (Anchor/Rust) |
| `web` | `fbca04` | `client/**` (Next.js) |
| `ci` | `e99695` | Workflows, repo config |
| `dependencies` | `0366d6` | Dependabot PRs |
| `blocked-on-counsel` | `d93f0b` | Waiting on a Q1–Q20 gate — exempt from stale bot |
| `blocked-on-issuer` | `d93f0b` | Waiting on issuer consent (K1/H1) — exempt from stale bot |
| `pinned` | `ffffff` | Never auto-close |
| `stale` | `ededed` | Set by the stale bot |

## Milestones (mirror doc 08 §12)

- `P0 — Scaffold`
- `P1 — Gate + Asset (AC-1)`
- `P2 — Marks + Treasury (AC-2)`
- `P3 — Auction (AC-3/4/5/11)`
- `P4 — Claims + Facility (AC-8/9)`
- `P5 — Tranching (AC-10)`
- `P6 — UI completion`
- `P7 — External audit`
- `P8 — Mainnet pilot (all ACs, recorded demo)`

## Definition of done (docs PR)

- Internal links resolve (CI: `docs/links`)
- Mermaid fences render (CI: `docs/mermaid`)
- No ID renumbering (CI: `docs/id-anchors`)
- New factual claims carry `[PRIMARY]/[SECONDARY]/[UNVERIFIED]` + dated source
