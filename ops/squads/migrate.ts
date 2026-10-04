/**
 * Squads v4 2-of-2 authority migration (SRS 08 §8, #126).
 *
 *   tsx migrate.ts create                                   # multisig + vault, record written
 *   tsx migrate.ts move-program <program> --authority <kp>  # upgrade authority → vault
 *   tsx migrate.ts move-mint    <mint>    --authority <kp>  # NPCS: mint, freeze, delegate,
 *                                                           #   metadata pointer + update → vault
 *   tsx migrate.ts prove                                    # vault tx: propose → 2 approvals
 *                                                           #   (→ execute once the timelock allows)
 *   tsx migrate.ts verify                                   # keyless: every recorded move holds
 *
 * Env: CLUSTER (devnet | localnet), RPC_URL (overrides), PAYER (fee payer
 * keypair, default ~/.config/solana/id.json), TIMELOCK_SECONDS (default
 * 172800 = 48h, SRS 08 §8), KEYS_DIR (default ./keys, gitignored).
 *
 * The record (`deployment-<cluster>.json`) holds only public keys and
 * signatures. Member keypairs stay in KEYS_DIR.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  AuthorityType,
  createSetAuthorityInstruction,
  getExtensionData,
  ExtensionType,
  getMint,
  TOKEN_2022_PROGRAM_ID,
  getMetadataPointerState,
  getPermanentDelegate,
  getTokenMetadata,
} from "@solana/spl-token";
import { createUpdateAuthorityInstruction } from "@solana/spl-token-metadata";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  TransactionMessage,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import * as multisig from "@sqds/multisig";

export const BPF_UPGRADEABLE = new PublicKey("BPFLoaderUpgradeab1e11111111111111111111111");
export const SQUADS_V4 = multisig.PROGRAM_ID;

const CLUSTER = process.env.CLUSTER ?? "devnet";
const RPC =
  process.env.RPC_URL ?? (CLUSTER === "devnet" ? "https://api.devnet.solana.com" : "http://127.0.0.1:8899");
const KEYS = process.env.KEYS_DIR ?? join(import.meta.dirname, "keys");
const RECORD = process.env.RECORD ?? join(import.meta.dirname, `deployment-${CLUSTER}.json`);

export interface Move {
  kind: "program-upgrade-authority" | "mint-authorities";
  target: string;
  from: string;
  signature: string;
  at: string;
}

export interface Record {
  cluster: string;
  squadsProgram: string;
  multisig: string;
  vault: string;
  createKey: string;
  members: string[];
  threshold: number;
  timeLockSeconds: number;
  createSignature: string;
  moves: Move[];
  proofs: { transactionIndex: string; proposeSignature: string; approvals: string[]; executeSignature?: string }[];
}

export function loadKeypair(path: string): Keypair {
  const p = path.replace(/^~(?=\/)/, homedir());
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(p, "utf8"))));
}

function keypairAt(name: string, create: boolean): Keypair {
  const p = join(KEYS, `${name}.json`);
  if (existsSync(p)) return loadKeypair(p);
  if (!create) throw new Error(`missing ${p}`);
  mkdirSync(KEYS, { recursive: true, mode: 0o700 });
  const k = Keypair.generate();
  writeFileSync(p, JSON.stringify(Array.from(k.secretKey)), { mode: 0o600 });
  return k;
}

const readRecord = (): Record => JSON.parse(readFileSync(RECORD, "utf8"));
const writeRecord = (r: Record) => writeFileSync(RECORD, JSON.stringify(r, null, 2) + "\n");

/** BPF loader v3 `SetAuthority` (enum index 4): the new authority need not sign, so a PDA vault can receive it. */
export function setUpgradeAuthorityIx(program: PublicKey, current: PublicKey, next: PublicKey): TransactionInstruction {
  const [programData] = PublicKey.findProgramAddressSync([program.toBuffer()], BPF_UPGRADEABLE);
  return new TransactionInstruction({
    programId: BPF_UPGRADEABLE,
    keys: [
      { pubkey: programData, isSigner: false, isWritable: true },
      { pubkey: current, isSigner: true, isWritable: false },
      { pubkey: next, isSigner: false, isWritable: false },
    ],
    data: Buffer.from([4, 0, 0, 0]),
  });
}

/** ProgramData header: u32 tag (3) | u64 slot | Option<Pubkey> authority. */
export async function upgradeAuthority(conn: Connection, program: PublicKey): Promise<string | null> {
  const [programData] = PublicKey.findProgramAddressSync([program.toBuffer()], BPF_UPGRADEABLE);
  const a = await conn.getAccountInfo(programData, "confirmed");
  if (!a) throw new Error(`no ProgramData for ${program.toBase58()}`);
  return a.data[12] === 1 ? new PublicKey(a.data.subarray(13, 45)).toBase58() : null;
}

/** Token ACL (sRFC37). Once a mint is wired to credit-gate (ops/gate
 * `wire.ts acl`), its freeze authority is this program's MintConfig PDA,
 * not a wallet: that is the target state, so a move leaves it there. */
export const TOKEN_ACL = new PublicKey("TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP");
export const aclMintConfig = (mint: PublicKey) =>
  PublicKey.findProgramAddressSync([Buffer.from("MINT_CONFIG"), mint.toBuffer()], TOKEN_ACL)[0].toBase58();

/** Is `holder` where authority `k` of `mint` should end up after a move? */
export const custodied = (k: string, holder: string | null, vault: string, mint: PublicKey) =>
  k === "hasMetadataPointer" || holder === null || holder === vault || (k === "freeze" && holder === aclMintConfig(mint));

/** Every authority on an NPCS-shaped Token-2022 mint. */
export async function mintAuthorities(conn: Connection, mint: PublicKey) {
  const m = await getMint(conn, mint, "confirmed", TOKEN_2022_PROGRAM_ID);
  const pointer = getMetadataPointerState(m);
  const meta = await getTokenMetadata(conn, mint, "confirmed", TOKEN_2022_PROGRAM_ID);
  return {
    mint: m.mintAuthority?.toBase58() ?? null,
    freeze: m.freezeAuthority?.toBase58() ?? null,
    permanentDelegate: getPermanentDelegate(m)?.delegate.toBase58() ?? null,
    metadataPointer: pointer?.authority?.toBase58() ?? null,
    metadataUpdate: meta?.updateAuthority?.toBase58() ?? null,
    hasMetadataPointer: !!getExtensionData(ExtensionType.MetadataPointer, m.tlvData),
  };
}

async function send(conn: Connection, ixs: TransactionInstruction[], signers: Keypair[]): Promise<string> {
  return sendAndConfirmTransaction(conn, new Transaction().add(...ixs), signers, { commitment: "confirmed" });
}

async function create(conn: Connection, payer: Keypair): Promise<void> {
  if (existsSync(RECORD)) throw new Error(`${RECORD} exists: a multisig is already recorded for ${CLUSTER}`);
  const [m1, m2, createKey] = ["member-1", "member-2", "create-key"].map((n) => keypairAt(n, true));
  const [multisigPda] = multisig.getMultisigPda({ createKey: createKey.publicKey });
  const [vault] = multisig.getVaultPda({ multisigPda, index: 0 });
  const [configPda] = multisig.getProgramConfigPda({});
  const programConfig = await multisig.accounts.ProgramConfig.fromAccountAddress(conn, configPda);
  const timeLock = Number(process.env.TIMELOCK_SECONDS ?? 172_800);
  const signature = await multisig.rpc.multisigCreateV2({
    connection: conn,
    treasury: programConfig.treasury,
    createKey,
    creator: payer,
    multisigPda,
    configAuthority: null, // autonomous: only a 2-of-2 vote changes members or threshold
    threshold: 2,
    members: [m1, m2].map((k) => ({ key: k.publicKey, permissions: multisig.types.Permissions.all() })),
    timeLock,
    rentCollector: null,
    sendOptions: { skipPreflight: false },
  });
  await conn.confirmTransaction(signature, "confirmed");
  writeRecord({
    cluster: CLUSTER,
    squadsProgram: SQUADS_V4.toBase58(),
    multisig: multisigPda.toBase58(),
    vault: vault.toBase58(),
    createKey: createKey.publicKey.toBase58(),
    members: [m1, m2].map((k) => k.publicKey.toBase58()),
    threshold: 2,
    timeLockSeconds: timeLock,
    createSignature: signature,
    moves: [],
    proofs: [],
  });
  console.log(`multisig ${multisigPda.toBase58()}\nvault    ${vault.toBase58()}\ntx       ${signature}`);
}

async function moveProgram(conn: Connection, program: PublicKey, authority: Keypair): Promise<void> {
  const r = readRecord();
  const before = await upgradeAuthority(conn, program);
  if (before !== authority.publicKey.toBase58()) {
    throw new Error(`upgrade authority is ${before}, not the signer ${authority.publicKey.toBase58()}`);
  }
  const sig = await send(conn, [setUpgradeAuthorityIx(program, authority.publicKey, new PublicKey(r.vault))], [authority]);
  const after = await upgradeAuthority(conn, program);
  if (after !== r.vault) throw new Error(`after move the authority is ${after}, expected the vault`);
  r.moves.push({ kind: "program-upgrade-authority", target: program.toBase58(), from: before, signature: sig, at: new Date().toISOString() });
  writeRecord(r);
  console.log(`${program.toBase58()} upgrade authority ${before} → vault ${r.vault}\ntx ${sig}`);
}

async function moveMint(conn: Connection, mint: PublicKey, authority: Keypair): Promise<void> {
  const r = readRecord();
  const vault = new PublicKey(r.vault);
  const a = await mintAuthorities(conn, mint);
  const me = authority.publicKey.toBase58();
  const ixs: TransactionInstruction[] = [];
  const set = (type: AuthorityType) =>
    ixs.push(createSetAuthorityInstruction(mint, authority.publicKey, type, vault, [], TOKEN_2022_PROGRAM_ID));
  // Move only what this key holds; refuse a partial custody state silently.
  const held: [string | null, () => void, string][] = [
    [a.mint, () => set(AuthorityType.MintTokens), "mint"],
    [a.freeze, () => set(AuthorityType.FreezeAccount), "freeze"],
    [a.permanentDelegate, () => set(AuthorityType.PermanentDelegate), "permanent delegate"],
    [a.metadataPointer, () => set(AuthorityType.MetadataPointer), "metadata pointer"],
    [
      a.metadataUpdate,
      () =>
        ixs.push(
          createUpdateAuthorityInstruction({
            programId: TOKEN_2022_PROGRAM_ID,
            metadata: mint,
            oldAuthority: authority.publicKey,
            newAuthority: vault,
          }),
        ),
      "metadata update",
    ],
  ];
  for (const [holder, add, name] of held) {
    if (holder === null) continue;
    if (name === "freeze" && holder === aclMintConfig(mint)) continue; // already with Token ACL
    if (holder !== me) throw new Error(`${name} authority is ${holder}, not the signer ${me}`);
    add();
  }
  if (ixs.length === 0) throw new Error("the signer holds no authority on this mint");
  const sig = await send(conn, ixs, [authority]);
  const after = await mintAuthorities(conn, mint);
  for (const [k, v] of Object.entries(after)) {
    if (!custodied(k, v as string | null, r.vault, mint)) throw new Error(`after move ${k} authority is ${v}`);
  }
  r.moves.push({ kind: "mint-authorities", target: mint.toBase58(), from: me, signature: sig, at: new Date().toISOString() });
  writeRecord(r);
  console.log(`${mint.toBase58()} authorities → vault ${r.vault}\ntx ${sig}`);
}

/**
 * Prove the vault answers only to 2-of-2: a vault transaction (a 0-lamport
 * self-transfer memo-equivalent) is proposed, approved by both members and,
 * when the timelock allows, executed. With the 48h timelock the execute
 * step is a separate later run (`prove` resumes the pending one).
 */
async function prove(conn: Connection, payer: Keypair): Promise<void> {
  const r = readRecord();
  const multisigPda = new PublicKey(r.multisig);
  const vault = new PublicKey(r.vault);
  const [m1, m2] = ["member-1", "member-2"].map((n) => keypairAt(n, false));
  const pending = r.proofs.find((p) => !p.executeSignature);
  let index: bigint;
  if (pending) {
    index = BigInt(pending.transactionIndex);
  } else {
    const ms = await multisig.accounts.Multisig.fromAccountAddress(conn, multisigPda);
    index = BigInt(ms.transactionIndex.toString()) + 1n;
    const message = new TransactionMessage({
      payerKey: vault,
      recentBlockhash: (await conn.getLatestBlockhash()).blockhash,
      instructions: [SystemProgram.transfer({ fromPubkey: vault, toPubkey: vault, lamports: 0 })],
    });
    const create = await multisig.rpc.vaultTransactionCreate({
      connection: conn, feePayer: payer, multisigPda, transactionIndex: index, creator: m1.publicKey, rentPayer: payer.publicKey,
      vaultIndex: 0, ephemeralSigners: 0, transactionMessage: message, signers: [payer, m1],
    });
    await conn.confirmTransaction(create, "confirmed");
    // Not rpc.proposalCreate: in @sqds/multisig 2.1.4 it drops `rentPayer`,
    // so the member would pay the proposal rent. Members hold no SOL.
    const propose = await send(
      conn,
      [multisig.instructions.proposalCreate({ multisigPda, transactionIndex: index, creator: m1.publicKey, rentPayer: payer.publicKey })],
      [payer, m1],
    );
    const approvals: string[] = [];
    for (const m of [m1, m2]) {
      const s = await multisig.rpc.proposalApprove({ connection: conn, feePayer: payer, member: m, multisigPda, transactionIndex: index });
      await conn.confirmTransaction(s, "confirmed");
      approvals.push(s);
    }
    r.proofs.push({ transactionIndex: index.toString(), proposeSignature: propose, approvals });
    writeRecord(r);
    console.log(`proposal #${index}: created, approved by both members`);
  }
  const [proposalPda] = multisig.getProposalPda({ multisigPda, transactionIndex: index });
  const proposal = await multisig.accounts.Proposal.fromAccountAddress(conn, proposalPda);
  const status = proposal.status;
  if (!multisig.types.isProposalStatusApproved(status)) throw new Error(`proposal #${index} is ${status.__kind}, not Approved`);
  const approvedAt = Number(status.timestamp);
  const ready = approvedAt + r.timeLockSeconds;
  const now = Math.floor(Date.now() / 1000);
  if (now < ready) {
    console.log(`approved at ${new Date(approvedAt * 1000).toISOString()}; timelock opens ${new Date(ready * 1000).toISOString()}. Re-run prove then to execute.`);
    return;
  }
  const exec = await multisig.rpc.vaultTransactionExecute({
    connection: conn, feePayer: payer, multisigPda, transactionIndex: index, member: m1.publicKey, signers: [payer, m1],
  });
  await conn.confirmTransaction(exec, "confirmed");
  const rec = readRecord();
  rec.proofs.find((p) => p.transactionIndex === index.toString())!.executeSignature = exec;
  writeRecord(rec);
  console.log(`proposal #${index} executed: ${exec}`);
}

/** Keyless: re-read every recorded fact from chain and fail on any drift. */
export async function verify(conn: Connection, r: Record): Promise<string[]> {
  const problems: string[] = [];
  const ms = await multisig.accounts.Multisig.fromAccountAddress(conn, new PublicKey(r.multisig));
  const members = ms.members.map((m) => m.key.toBase58()).sort();
  if (ms.threshold !== 2 || members.length !== 2) problems.push(`multisig is ${ms.threshold}-of-${members.length}, want 2-of-2`);
  if (members.join() !== [...r.members].sort().join()) problems.push(`members ${members} ≠ recorded ${r.members}`);
  if (ms.timeLock !== r.timeLockSeconds) problems.push(`timelock ${ms.timeLock}s ≠ recorded ${r.timeLockSeconds}s`);
  if (ms.configAuthority.toBase58() !== PublicKey.default.toBase58()) problems.push(`config authority ${ms.configAuthority} can bypass the vote`);
  const [vault] = multisig.getVaultPda({ multisigPda: new PublicKey(r.multisig), index: 0 });
  if (vault.toBase58() !== r.vault) problems.push(`vault ${vault} ≠ recorded ${r.vault}`);
  for (const mv of r.moves) {
    if (mv.kind === "program-upgrade-authority") {
      const a = await upgradeAuthority(conn, new PublicKey(mv.target));
      if (a !== r.vault) problems.push(`${mv.target} upgrade authority is ${a}`);
    } else {
      const mint = new PublicKey(mv.target);
      for (const [k, v] of Object.entries(await mintAuthorities(conn, mint))) {
        if (!custodied(k, v as string | null, r.vault, mint)) problems.push(`${mv.target} ${k} authority is ${v}`);
      }
    }
  }
  return problems;
}

async function main(): Promise<void> {
  const [cmd, arg] = process.argv.slice(2);
  const flag = (n: string) => {
    const i = process.argv.indexOf(`--${n}`);
    return i > 0 ? process.argv[i + 1] : undefined;
  };
  const conn = new Connection(RPC, "confirmed");
  const payer = () => loadKeypair(process.env.PAYER ?? "~/.config/solana/id.json");
  const need = (v: string | undefined, what: string) => {
    if (!v) throw new Error(`${what} is required`);
    return v;
  };
  switch (cmd) {
    case "create":
      return create(conn, payer());
    case "move-program":
      return moveProgram(conn, new PublicKey(need(arg, "<program>")), loadKeypair(need(flag("authority"), "--authority")));
    case "move-mint":
      return moveMint(conn, new PublicKey(need(arg, "<mint>")), loadKeypair(need(flag("authority"), "--authority")));
    case "prove":
      return prove(conn, payer());
    case "verify": {
      const problems = await verify(conn, readRecord());
      if (problems.length) throw new Error(`verify FAILED:\n  ${problems.join("\n  ")}`);
      const r = readRecord();
      console.log(`verify OK — ${r.multisig} is 2-of-2 (timelock ${r.timeLockSeconds}s); ${r.moves.length} authority move(s) hold`);
      return;
    }
    default:
      throw new Error("usage: migrate.ts <create|move-program|move-mint|prove|verify> — see the header");
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    console.error(e instanceof Error ? e.message : e);
    // A failed transaction's program logs say why; print them.
    const logs = (e as { logs?: string[]; transactionLogs?: string[] }).logs ?? (e as { transactionLogs?: string[] }).transactionLogs;
    if (logs) console.error(logs.join("\n"));
    process.exit(1);
  });
}
