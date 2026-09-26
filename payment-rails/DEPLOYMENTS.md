# payment-rails deployments

Every row is verifiable: `solana program dump <id> x.so -u <cluster>` →
`sha256sum x.so` must equal the Build SHA-256; `solana program show <id>`
must show the Upgrade authority. Recorded by `crank/scripts/devnet/deploy.sh`.

| Program | Cluster | Program ID | Slot | Signature | Build SHA-256 | Upgrade authority |
|---|---|---|---|---|---|---|
| payment-rails v0.1.0 (deploy) | devnet | `GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn` | 504436527 | `3aWLZJ9WegXPJXkTrQHY2hpmTTm8PQc3Ztmd1EENAXxSc3AgHAfZL6yMqspA1QRpkEmi2XeD27c3sHtRwU5ioR3w` | `35ace429088050215fdb682ac9dcabc7834e93d50af73d41cbc7a30f4bcb7f38` | `9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ` (single key, pilot; Squads 2-of-2 per SRS §8 before mainnet) |

## R5 devnet demo (exit criterion: commit → settle → payout on devnet)

Settlement mint: Circle devnet USDC `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU` · config `HxQznN6P3NjKBkyi3UGz1ePJjop76mHNiWP9kFtKKYyc` · fees 10 bps + 5 bps reserve.
Session `3HRgupL1CgpFEcQqVpQu8etzdKsgBL4V58o6ike7tmVN` · facility `9Hd7Sz3H5phosAEbduLDzJpKWXDnHrvfoi61cvVnmASv` · reconciliation clean (0 open escrows, 2 receipts, seller + fee + reserve = gross) · evidence: 47 events, manifest SHA-256 `fc1c7bc7586e9fafa0eb53339a0c2550794eaa312daeefdef7c163c3cc99ad53`.

| Step | Transaction |
|---|---|
| Payout registry: H admitted (hash-only attestation) | [`67ir1pPJ…`](https://explorer.solana.com/tx/67ir1pPJT3aMRPL3fcLf5opmgxhNTkRNeajQkGMhS6p38ghBwPk1d3AWNW2pHUX78XfQ6szceUxKMAjnT3f9gwac?cluster=devnet) |
| Facility advance 2.0 USDC → H's registered wallet | [`4uVaVakG…`](https://explorer.solana.com/tx/4uVaVakGzMSHdaDZjrfATvdC7psqpYgfJN1LLLk9Sb1b3XAt7765KWCRnVZKXCeQ1NcNe96UPBdK6HMeyvtJpm4?cluster=devnet) |
| Commit: H ask (open_asset_vault + deposit_ask) | [`57xjJibR…`](https://explorer.solana.com/tx/57xjJibRhzkcDmpgxyMBsdiQMHwTuf38sLtPZitMED9CteD6NqcLCmvZ29EgHVDR2PL2rwDKomMCpTUAtFzBqmPX?cluster=devnet) |
| Commit: X bid 2.0 USDC | [`522Ski6r…`](https://explorer.solana.com/tx/522Ski6r2papg5qwcqAVzHdLSabGYYBtR8njQRgiaBVYmo3BL2Mrok43pNgFC3SgF4QKXSQoRSLHh7uFbi8UMRFK?cluster=devnet) |
| Commit: Y bid 1.5 USDC | [`2hJwtDmH…`](https://explorer.solana.com/tx/2hJwtDmHjjPtSpr22ZS3v9AbRzp6qzbbXcCMWirKmb4Y4mfCZP3vJLXJRmcPqiJ8Fj19wY7bcixVizZ9pX1FbG4F?cluster=devnet) |
| DvP fill #0 (0.6 @ 3.0) | [`2Ug61kJu…`](https://explorer.solana.com/tx/2Ug61kJuqnueQscNjgU1TfEPHJnc72jccYHBe6BV62LzLaLCxDXVCRVkoYJh995EcR8LFS6QNs64BeBdaKpDwurD?cluster=devnet) |
| DvP fill #1 (0.4 @ 3.0) | [`2AWZxiiJ…`](https://explorer.solana.com/tx/2AWZxiiJWBzbGq22tVUBrLjzKPNNESa4ASU6Ep815AaNNa1Hm6ZUnFh2vzHFHd1HMJFmowmvkfRKe6hbSbXfbPxB?cluster=devnet) |
| Finish settlement | [`Dgqs2fhb…`](https://explorer.solana.com/tx/Dgqs2fhbaeuuEA6GWwWhGDHudc4mjiBgdj9EesFeZ2GF9EXAakh9Dh2NWCccP9SRAJUWXm1Xa9js1Lzn719FT8Q?cluster=devnet) |
| Refunds (excess cash + bonds, by the crank) | [`2fCQXiyu…`](https://explorer.solana.com/tx/2fCQXiyuc8BAHE6YbnCEs3qciW4RMELG7kDfpWFiP31L4ZAN6airrYrr4KHfKYkQ1o71KkGqS1u7aXWAjnn1aG7E?cluster=devnet) · [`5CE4gEvG…`](https://explorer.solana.com/tx/5CE4gEvGb1WHHfJpcR5ixUb1SN1JiK3cKMCTqKrgVmT7dfReDHZiRLHKbQNgFXbVEygdzW34rGz53Vxajr59Jhso?cluster=devnet) · [`3efYDPKq…`](https://explorer.solana.com/tx/3efYDPKqHRE4vCApF97jEsickrvc2WjHjY4iyap1MGV6mTCwNV3SahR6my6VhgNfhFnGEZ4HrZuLU9fHqxnsWyMQ?cluster=devnet) |
| Payout sweep: proceeds 2.9955 → facility 2.0 first, H 0.9955 | [`48w6iyhZ…`](https://explorer.solana.com/tx/48w6iyhZJues5hQ48hAisLpbitXieYRpBxxMkvEbzZeoJznYtd8tU7vSbJSpT4obzDzB1eKPVq8jKWWsGYNUN93a?cluster=devnet) |

An earlier run (session `7Nz2FyNXuDjFcBGvrCCwpRUPoCcB35x33eqvghFZJRAA`) lost fill #1's confirmation to a network error. The retry hit the fill receipt and was rejected (idempotent), and the stand-alone crank CLI then finished, refunded and reconciled that session cleanly on devnet.
