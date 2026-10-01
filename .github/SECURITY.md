# Security Policy

NUNDINA is pre-launch (preresearch + pilot). **Programs are live on Solana devnet only**:
`payment-rails` (the USDC cash layer that holds every escrowed client dollar), the
`credit-gate` skeleton and the NPCS pilot mint (`DEPLOYMENTS.md`,
`payment-rails/DEPLOYMENTS.md`). Nothing is deployed on mainnet and no
third-party funds are accepted. Devnet bugs still count: the same code is what
would go to mainnet.

## Responsible disclosure

If you find a vulnerability in any design document, specification, program,
script or app code:

1. **Do not open a public issue.**
2. Email **security@nundina.finance** (or contact the maintainers privately via GitHub) with:
   - affected doc/program and version or commit hash,
   - a minimal reproduction or attack narrative,
   - your assessment of severity (we use S1–S4 as defined in `preresearch/architecture/05-Problem-Register.md`).
3. We will acknowledge within 72 hours and keep you informed through resolution.

## Scope

- In scope:
  - `payment-rails/` — the Anchor program, its LiteSVM tests, and the crank /
    reconciliation / evidence tooling and deploy scripts under `crank/`. This
    is the highest-priority target: it holds all client cash (SRS 08 §4.7),
    including the permissionless refund instructions.
  - `programs/` — `credit-gate` and `credit-math`.
  - `token/scripts/` — the NPCS mint initialisation and verification scripts.
  - `app/` — the Next.js app.
  - `preresearch/` — design flaws count; this is a securities-infrastructure design.
  - Key custody and deployment procedures documented in this repo
    (`DEPLOYMENTS.md`, `payment-rails/README.md` § Keys, `token/runbook.md`).
- Out of scope: social engineering, denial-of-service against infrastructure,
  spam, and findings that only restate a limitation the docs already disclose
  as not built (e.g. the as-built notes in SRS 08 §3/§5/§6).

## Safe harbor

We will not pursue legal action against good-faith research that respects this policy.
