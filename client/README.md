# NUNDINA — client (marketing site)

Next.js 16 (App Router, `src/`, TypeScript, Tailwind v4). The public landing page; the live
chain-reading dApp lives in `../app` and is linked under `/app`.

```bash
npm install
npm run dev        # http://localhost:3000
npm run build && npm start
```

Optional env:

| Var | Default | Purpose |
|---|---|---|
| `NEXT_PUBLIC_APP_URL` | `/app` | Where "Enter Nundina" / app links point |
| `NEXT_PUBLIC_SOLANA_RPC_URL` | `https://api.devnet.solana.com` | Live epoch/slot in the telemetry strip |

Layout: `src/components/{layout,landing,ui}`; all routes in `src/lib/site.ts`; every number
shown (with its source) in `src/lib/data.ts`. Content rules follow `frontend/info.md`: devnet
facts only, cited market data, no APY/TVL/audit claims.
