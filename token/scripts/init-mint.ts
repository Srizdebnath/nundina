/**
 * NPCS mint initialization — doc 09 §2 extension manifest, executed for real.
 *
 * Mechanics (verified against spl-token-2022 source + three empirical runs):
 *
 *  1. InitializeMint2 requires the account length to EXACTLY equal the size
 *     of the extensions initialized so far → mint is created pods-only
 *     (getTokenSize of the 3 pod extensions = 275 bytes).
 *  2. Metadata Initialize/UpdateField realloc the account internally but do
 *     NOT fund it → a SystemProgram.transfer of the rent delta precedes each.
 *  3. The realloc CANNOT happen in the same transaction as createAccount
 *     ("memory allocation failed, out of memory" — SBF panic) → metadata
 *     operations run in SEPARATE transactions, exactly like the official
 *     spl-token helpers (tokenMetadataInitializeWithRentTransfer et al).
 *  4. Self-pointer is mandatory: "Metadata for a mint must be initialized
 *     in the mint itself."
 *
 * Flow:
 *   tx1: createAccount(275) → DAS(Frozen) → PD → MP(self) → IM2
 *   tx2: [transfer Δrent] metaInitialize          (realloc → 428)
 *   tx3…n: per label pair: [transfer Δrent] UpdateField
 *
 * Keypairs: asset/npcs/keys/*.json (gitignored) — back up before cargo clean.
 * Bootstrap authority is single-key; Squads 2-of-2 takes over at P1 (08 §8).
 */
import {
  Address,
  appendTransactionMessageInstructions,
  createKeyPairSignerFromBytes,
  createSolanaRpc,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  getUtf8Encoder,
  KeyPairSigner,
  partiallySignTransactionMessageWithSigners,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  type Instruction,
} from "@solana/kit";
import {
  AccountState,
  getInitializeDefaultAccountStateInstruction,
  getInitializeMint2Instruction,
  getInitializeMetadataPointerInstruction,
  getInitializePermanentDelegateInstruction,
  getTokenSize,
  TOKEN_2022_PROGRAM_ADDRESS,
} from "@solana-program/token-2022";
import { getCreateAccountInstruction, getTransferSolInstruction } from "@solana-program/system";
import { createUpdateFieldInstruction, pack } from "@solana/spl-token-metadata";
import { PublicKey, type TransactionInstruction as Web3Instruction } from "@solana/web3.js";
import { readFile, writeFile } from "node:fs/promises";

// ── NPCS facts (params.toml / doc 09 D-B) ──────────────────────────────
const NAME = "NUNDINA Pilot Credit Sleeve";
const SYMBOL = "NPCS";
const DECIMALS = 6;
const URI = "https://nundina.com/npcs/metadata.json";
const LABEL = "PILOT-DEMO-ASSET — NOT AN OFFER — SYNTHETIC LABEL";
const LABELS: [string, string][] = [
  ["label", LABEL],
  ["asset_class", "pilot_credit_sleeve"],
  ["nav_precision", "pico"],
  ["backing", "USDC_1to1"],
];

// spl_token_metadata_interface discriminators (installed package source).
const META_INIT_DISC = new Uint8Array([210, 225, 30, 162, 88, 184, 77, 141]);
const META_UPDATE_FIELD_DISC = new Uint8Array([221, 233, 49, 45, 181, 202, 220, 200]);

const CLUSTER = (process.env.CLUSTER ?? "devnet") as "devnet" | "mainnet" | "local";
const RPC_URL =
  process.env.RPC_URL ??
  (CLUSTER === "mainnet"
    ? "https://api.mainnet-beta.solana.com"
    : "https://api.devnet.solana.com");

const u32le = (n: number): Uint8Array => {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n, true);
  return b;
};
const utf8 = (s: string): Uint8Array => new Uint8Array(getUtf8Encoder().encode(s));
const borshString = (s: string): Uint8Array[] => {
  const b = utf8(s);
  return [u32le(b.length), b];
};
const concat = (parts: Uint8Array[]): Uint8Array => {
  const len = parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(len);
  let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
};

// Convert a web3.js TransactionInstruction to a kit Instruction (the official
// spl-token-metadata builders are web3-generation; their DATA is what we want).
function fromWeb3(ix: Web3Instruction): Instruction {
  return {
    programAddress: ix.programId.toBase58() as Address,
    accounts: ix.keys.map((k) => ({
      address: k.pubkey.toBase58() as Address,
      role: (k.isWritable ? 1 : 0) | (k.isSigner ? 2 : 0),
    })),
    data: new Uint8Array(ix.data),
  };
}

async function loadKeypairSigner(path: string): Promise<KeyPairSigner> {
  const bytes = new Uint8Array(JSON.parse(await readFile(path, "utf8")) as number[]);
  return createKeyPairSignerFromBytes(bytes);
}

async function main(): Promise<void> {
  const rpc = createSolanaRpc(RPC_URL);
  const rent = async (len: number) =>
    Number(await rpc.getMinimumBalanceForRentExemption(BigInt(len)).send());

  async function sendTx(ixs: Instruction[], label: string): Promise<string> {
    const { value: { blockhash, lastValidBlockHeight } } = await rpc.getLatestBlockhash().send();
    const message = pipe(
      createTransactionMessage({ version: 0 }),
      (m) => setTransactionMessageLifetimeUsingBlockhash({ blockhash, lastValidBlockHeight }, m),
      (m) => setTransactionMessageFeePayerSigner(admin, m),
      (m) => appendTransactionMessageInstructions(ixs, m),
    );
    const signed = await partiallySignTransactionMessageWithSigners(message);
    const wire = getBase64EncodedWireTransaction(signed);
    const signature = await rpc
      .sendTransaction(wire, { encoding: "base64", preflightCommitment: "confirmed" })
      .send();
    for (let i = 0; i < 30; i++) {
      const { value: statuses } = await rpc.getSignatureStatuses([signature]).send();
      const st = statuses[0];
      if (st) {
        if (st.err) throw new Error(`[${label}] tx failed: ${JSON.stringify(st.err)}`);
        if (st.confirmationStatus === "confirmed" || st.confirmationStatus === "finalized") break;
      }
      await new Promise((r) => setTimeout(r, 1_000));
    }
    console.log(`  ${label}: ${signature.slice(0, 20)}… ✓`);
    return signature;
  }

  const mint = await loadKeypairSigner("keys/mint.json");
  const admin = await loadKeypairSigner("keys/freeze-authority.json");
  const mintAddr = mint.address as Address;

  console.log(`cluster:      ${CLUSTER}`);
  console.log(`mint:         ${mint.address}`);
  console.log(`authority:    ${admin.address}`);

  const packedLen = (pairs: [string, string][]): number =>
    pack({
      updateAuthority: PublicKey.default,
      mint: PublicKey.default,
      name: NAME,
      symbol: SYMBOL,
      uri: URI,
      additionalMetadata: pairs.map(([k, v]) => [k, v] as const),
    }).length;

  // ── tx1: pods-only, exact-fit account, fully initialized ────────────
  const podSpace = getTokenSize([
    { __kind: "MetadataPointer", authority: admin.address, metadataAddress: mintAddr },
    { __kind: "PermanentDelegate", delegate: admin.address },
    { __kind: "DefaultAccountState", state: AccountState.Frozen },
  ]);
  const podLamports = await rent(podSpace);
  console.log(`pod space:    ${podSpace} (exact fit — IM2's invariant)`);

  await sendTx(
    [
      getCreateAccountInstruction({
        payer: admin,
        newAccount: mint,
        space: BigInt(podSpace),
        lamports: BigInt(podLamports),
        programAddress: TOKEN_2022_PROGRAM_ADDRESS,
      }),
      getInitializeDefaultAccountStateInstruction({ mint: mint.address, state: AccountState.Frozen }),
      getInitializePermanentDelegateInstruction({ mint: mint.address, delegate: admin.address }),
      getInitializeMetadataPointerInstruction({
        mint: mint.address,
        authority: admin.address,
        metadataAddress: mintAddr,
      }),
      getInitializeMint2Instruction({
        mint: mint.address,
        decimals: DECIMALS,
        mintAuthority: admin.address,
        freezeAuthority: admin.address,
      }),
    ],
    "tx1 mint+pods",
  );

  // ── tx2: metadata base fields (own tx — realloc + rent top-up) ──────
  const baseTlv = 4 + packedLen([]);
  const afterBase = podSpace + baseTlv;
  const topUpBase = (await rent(afterBase)) - podLamports;
  console.log(`metadata TLV: base ${baseTlv}B → ${afterBase}B (top-up ${topUpBase} lamports)`);

  const metaIxs: Instruction[] = [];
  if (topUpBase > 0) {
    metaIxs.push(
      getTransferSolInstruction({
        source: admin,
        destination: mintAddr,
        amount: BigInt(topUpBase),
      }) as unknown as Instruction,
    );
  }
  metaIxs.push({
    programAddress: TOKEN_2022_PROGRAM_ADDRESS,
    accounts: [
      { address: mintAddr, role: 1 }, // metadata == mint (writable)
      { address: admin.address, role: 0 }, // update authority
      { address: mintAddr, role: 0 }, // mint
      { address: admin.address, role: 2 }, // mint authority (signer)
    ],
    data: concat([META_INIT_DISC, ...borshString(NAME), ...borshString(SYMBOL), ...borshString(URI)]),
  } satisfies Instruction);
  await sendTx(metaIxs, "tx2 meta-init");

  // ── tx3…: label pairs, one tx each (own realloc + rent top-up) ──────
  // UpdateField rewrites the FULL metadata (base + ALL pairs so far), so the
  // size target must accumulate pairs, not treat each as the only one.
  let currentLen = afterBase;
  const accumulated: [string, string][] = [];
  for (const [k, v] of LABELS) {
    accumulated.push([k, v]);
    const targetLen = podSpace + 4 + packedLen(accumulated);
    const delta = (await rent(targetLen)) - (await rent(currentLen));
    currentLen = targetLen;
    const ixs: Instruction[] = [];
    if (delta > 0) {
      ixs.push(
        getTransferSolInstruction({
          source: admin,
          destination: mintAddr,
          amount: BigInt(delta),
        }) as unknown as Instruction,
      );
    }
    // Official builder encodes the Key-variant layout — no hand-rolling.
    ixs.push(
      fromWeb3(
        createUpdateFieldInstruction({
          programId: new PublicKey(TOKEN_2022_PROGRAM_ADDRESS),
          metadata: new PublicKey(mintAddr),
          updateAuthority: new PublicKey(admin.address),
          field: k,
          value: v,
        }),
      ),
    );
    await sendTx(ixs, `tx label:${k}`);
  }
  console.log(`final size:   ${currentLen}B with ${LABELS.length} label pairs`);

  const deployment = {
    cluster: CLUSTER,
    mint: mint.address,
    metadata_layout: "token2022-tlv-self-pointer",
    admin_authority: admin.address,
    extensions: ["DefaultAccountState:Frozen", "MetadataPointer:self", "PermanentDelegate", "TokenMetadata"],
    pod_space: podSpace,
    final_space: currentLen,
    timestamp: new Date().toISOString(),
  };
  await writeFile(`keys/deployment-${CLUSTER}.json`, JSON.stringify(deployment, null, 2) + "\n");
  console.log(JSON.stringify(deployment, null, 2));
  console.log("\nNPCS mint initialized ✓");
}

main().catch((e) => {
  console.error("init-mint failed:", e);
  process.exit(1);
});
