/**
 * Squads v4 2-of-2 authority migration (SRS 08 §8, #126).
 *
 *   tsx migrate.ts create                                   # multisig + vault, record written
 *   tsx migrate.ts move-program <program> --authority <kp>  # upgrade authority → vault
 *   tsx migrate.ts move-mint    <mint>    --authority <kp>  # NPCS: mint, freeze, delegate,
 *                                                           #   metadata pointer + update → vault
 *   tsx migrate.ts prove                                    # vault tx: propose → 2 approvals
 *                                                           #   (→ execute once the timelock allows)
 *   tsx migrate.ts propose-upgrade <program> --buffer <buf> # vault tx: BPF Upgrade from a buffer
 *                       [--spill <addr>]                    #   whose authority is the vault →
 *                                                           #   propose + 2 approvals (#172)
 *   tsx migrate.ts execute-upgrade <program>                # once the timelock allows
 *   tsx migrate.ts propose-treasury <mint> --sleeve <usdc>  # vault tx: NPCS mint authority →
 *                                                           #   npcs-treasury (#148), initialize it
 *                                                           #   (authority = vault), move the backing
 *                                                           #   USDC in, update the metadata status
 *   tsx migrate.ts execute-treasury                         # once the timelock allows
 *   tsx migrate.ts verify                                   # keyless: every recorded move holds
 *
 * Env: CLUSTER (devnet | localnet), RPC_URL (overrides), PAYER (fee payer
 * keypair, default ~/.config/solana/id.json), TIMELOCK_SECONDS (default
 * 172800 = 48h, SRS 08 §8), KEYS_DIR (default ./keys, gitignored).
 *
 * The record (`deployment-<cluster>.json`) holds only public keys and
 * signatures. Member keypairs stay in KEYS_DIR.
 */
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  AuthorityType,
  createSetAuthorityInstruction,
  createTransferCheckedInstruction,
  getAccount,
  getExtensionData,
  ExtensionType,
  getMint,
  TOKEN_2022_PROGRAM_ID,
  getMetadataPointerState,
  getPermanentDelegate,
  getTokenMetadata,
} from "@solana/spl-token";
import { createUpdateAuthorityInstruction, createUpdateFieldInstruction } from "@solana/spl-token-metadata";
import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_CLOCK_PUBKEY,
  SYSVAR_RENT_PUBKEY,
  Transaction,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
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
  /** Program upgrades the vault signed (#172). Absent in older records. */
  upgrades?: Upgrade[];
  /** The NPCS mint authority handed to npcs-treasury (#148). */
  treasury?: TreasuryHandover;
}

export interface TreasuryHandover {
  program: string;
  mint: string;
  treasury: string;
  /** The treasury's signer PDA: the mint authority after execution. */
  signer: string;
  vault: string;
  usdcMint: string;
  sleeve: string;
  /** USDC moved from the sleeve into the treasury vault (= supply / scale: I1 holds at once). */
  amount: string;
  status: string;
  transactionIndex: string;
  proposeSignature: string;
  approvals: string[];
  executeSignature?: string;
}

export interface Upgrade {
  program: string;
  buffer: string;
  /** SHA-256 of the buffer's program bytes at proposal time. */
  bufferSha256: string;
  spill: string;
  transactionIndex: string;
  proposeSignature: string;
  approvals: string[];
  executeSignature?: string;
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
export const custodied = (k: string, holder: string | null, vault: string, mint: PublicKey, mintAuthority?: string) =>
  k === "hasMetadataPointer" ||
  holder === null ||
  holder === vault ||
  (k === "freeze" && holder === aclMintConfig(mint)) ||
  // After the treasury hand-over (#148) the mint authority is npcs-treasury's signer PDA, by design.
  (k === "mint" && mintAuthority !== undefined && holder === mintAuthority);

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
/** Create vault transaction `index` with `instructions`, propose it, and
 * have both members approve. Returns the proposal and approval signatures. */
async function proposeAndApprove(
  conn: Connection,
  payer: Keypair,
  r: Record,
  instructions: TransactionInstruction[],
): Promise<{ index: bigint; propose: string; approvals: string[] }> {
  const multisigPda = new PublicKey(r.multisig);
  const vault = new PublicKey(r.vault);
  const [m1, m2] = ["member-1", "member-2"].map((n) => keypairAt(n, false));
  const ms = await multisig.accounts.Multisig.fromAccountAddress(conn, multisigPda);
  const index = BigInt(ms.transactionIndex.toString()) + 1n;
  const message = new TransactionMessage({
    payerKey: vault,
    recentBlockhash: (await conn.getLatestBlockhash()).blockhash,
    instructions,
  });
  const create = await multisig.rpc.vaultTransactionCreate({
    connection: conn, feePayer: payer, multisigPda, transactionIndex: index, creator: m1!.publicKey, rentPayer: payer.publicKey,
    vaultIndex: 0, ephemeralSigners: 0, transactionMessage: message, signers: [payer, m1!],
  });
  await conn.confirmTransaction(create, "confirmed");
  // Not rpc.proposalCreate: in @sqds/multisig 2.1.4 it drops `rentPayer`,
  // so the member would pay the proposal rent. Members hold no SOL.
  const propose = await send(
    conn,
    [multisig.instructions.proposalCreate({ multisigPda, transactionIndex: index, creator: m1!.publicKey, rentPayer: payer.publicKey })],
    [payer, m1!],
  );
  const approvals: string[] = [];
  for (const m of [m1!, m2!]) {
    const s = await multisig.rpc.proposalApprove({ connection: conn, feePayer: payer, member: m, multisigPda, transactionIndex: index });
    await conn.confirmTransaction(s, "confirmed");
    approvals.push(s);
  }
  return { index, propose, approvals };
}

/** Execute vault transaction `index` if approved and past the timelock;
 * otherwise say when it opens and return null. */
async function executeWhenReady(conn: Connection, payer: Keypair, r: Record, index: bigint): Promise<string | null> {
  const multisigPda = new PublicKey(r.multisig);
  const [proposalPda] = multisig.getProposalPda({ multisigPda, transactionIndex: index });
  const proposal = await multisig.accounts.Proposal.fromAccountAddress(conn, proposalPda);
  const status = proposal.status;
  if (!multisig.types.isProposalStatusApproved(status)) throw new Error(`proposal #${index} is ${status.__kind}, not Approved`);
  const approvedAt = Number(status.timestamp);
  const ready = approvedAt + r.timeLockSeconds;
  if (Math.floor(Date.now() / 1000) < ready) {
    console.log(`proposal #${index} approved at ${new Date(approvedAt * 1000).toISOString()}; timelock opens ${new Date(ready * 1000).toISOString()}.`);
    return null;
  }
  const m1 = keypairAt("member-1", false);
  const { instruction, lookupTableAccounts } = await multisig.instructions.vaultTransactionExecute({
    connection: conn, multisigPda, transactionIndex: index, member: m1.publicKey,
  });
  const msg = new TransactionMessage({
    payerKey: payer.publicKey, recentBlockhash: (await conn.getLatestBlockhash()).blockhash,
    instructions: [ComputeBudgetProgram.setComputeUnitLimit({ units: 1_000_000 }), instruction],
  }).compileToV0Message(lookupTableAccounts);
  const tx = new VersionedTransaction(msg);
  tx.sign([payer, m1]);
  // Simulate first: a failed execute's program logs are the only clue.
  const sim = await conn.simulateTransaction(tx, { sigVerify: false });
  if (sim.value.err) throw new Error(`execute #${index} would fail: ${JSON.stringify(sim.value.err)}\n${(sim.value.logs ?? []).join("\n")}`);
  const exec = await conn.sendTransaction(tx);
  await conn.confirmTransaction(exec, "confirmed");
  return exec;
}

const BPF_LOADER = new PublicKey("BPFLoaderUpgradeab1e11111111111111111111111");
/** loader-v3 Buffer header: u32 tag (1) | Option<Pubkey> authority (33) → program bytes at 37. */
const BUFFER_DATA_AT = 37;
/** loader-v3 ProgramData header: u32 tag | u64 slot | Option<Pubkey> → 45. */
const PROGRAMDATA_HEADER = 45;

/** #172: a program upgrade signed by the vault. The buffer must already
 * hold the new build, with the vault as its authority
 * (`solana program write-buffer` + `set-buffer-authority`). */
async function proposeUpgrade(conn: Connection, payer: Keypair, program: PublicKey, buffer: PublicKey, spill: PublicKey): Promise<void> {
  const r = readRecord();
  const vault = new PublicKey(r.vault);
  if ((await upgradeAuthority(conn, program)) !== r.vault) throw new Error(`${program.toBase58()} upgrade authority is not the vault`);
  const b = await conn.getAccountInfo(buffer, "confirmed");
  if (!b || !b.owner.equals(BPF_LOADER) || b.data.readUInt32LE(0) !== 1) throw new Error(`${buffer.toBase58()} is not a loader-v3 buffer`);
  if (b.data[4] !== 1 || !new PublicKey(b.data.subarray(5, 37)).equals(vault)) throw new Error("buffer authority is not the vault (solana program set-buffer-authority)");
  const sha = createHash("sha256").update(b.data.subarray(BUFFER_DATA_AT)).digest("hex");
  const [programData] = PublicKey.findProgramAddressSync([program.toBuffer()], BPF_LOADER);
  // A larger build needs a larger ProgramData. The loader's legacy
  // ExtendProgram (6) is permissionless, so the operator pays for it before
  // proposing. Inside the vault transaction it would not work: Squads
  // executes by CPI, and a CPI may grow an account by only 10 KiB. Where
  // the cluster has replaced it with ExtendProgramChecked (feature
  // 2oMRZEDW…xwz; inactive on devnet as of 2026-10-04) this stops instead.
  const pd = await conn.getAccountInfo(programData, "confirmed");
  const extra = b.data.length - BUFFER_DATA_AT - ((pd?.data.length ?? 0) - PROGRAMDATA_HEADER);
  if (extra > 0) {
    const data = Buffer.alloc(8);
    data.writeUInt32LE(6, 0);
    data.writeUInt32LE(extra, 4);
    const m = (pubkey: PublicKey, isWritable: boolean, isSigner = false) => ({ pubkey, isWritable, isSigner });
    const legacy = new TransactionInstruction({
      programId: BPF_LOADER,
      keys: [m(programData, true), m(program, true), m(SystemProgram.programId, false), m(payer.publicKey, true, true)],
      data,
    });
    const probe = new Transaction().add(legacy);
    probe.feePayer = payer.publicKey;
    probe.recentBlockhash = (await conn.getLatestBlockhash()).blockhash;
    probe.sign(payer);
    const sim = await conn.simulateTransaction(probe);
    if (sim.value.err) {
      throw new Error(`ProgramData needs ${extra} more bytes and the permissionless ExtendProgram is refused:\n${(sim.value.logs ?? []).join("\n")}`);
    }
    console.log(`ProgramData extended by ${extra} bytes (paid by ${payer.publicKey.toBase58()}): ${await send(conn, [legacy], [payer])}`);
  }
  const k = (pubkey: PublicKey, isWritable: boolean, isSigner = false) => ({ pubkey, isWritable, isSigner });
  const upgrade = new TransactionInstruction({
    programId: BPF_LOADER,
    keys: [
      k(programData, true), k(program, true), k(buffer, true), k(spill, true),
      k(SYSVAR_RENT_PUBKEY, false), k(SYSVAR_CLOCK_PUBKEY, false), k(vault, false, true),
    ],
    data: Buffer.from([3, 0, 0, 0]), // UpgradeableLoaderInstruction::Upgrade
  });
  const { index, propose, approvals } = await proposeAndApprove(conn, payer, r, [upgrade]);
  r.upgrades = [...(r.upgrades ?? []), {
    program: program.toBase58(), buffer: buffer.toBase58(), bufferSha256: sha, spill: spill.toBase58(),
    transactionIndex: index.toString(), proposeSignature: propose, approvals,
  }];
  writeRecord(r);
  console.log(`upgrade proposal #${index} for ${program.toBase58()} (buffer ${buffer.toBase58()}, sha256 ${sha}): created, approved by both members`);
  await executeUpgrade(conn, payer, program);
}

async function executeUpgrade(conn: Connection, payer: Keypair, program: PublicKey): Promise<void> {
  const r = readRecord();
  const u = (r.upgrades ?? []).find((x) => x.program === program.toBase58() && !x.executeSignature);
  if (!u) throw new Error(`no pending upgrade of ${program.toBase58()} in the record`);
  const exec = await executeWhenReady(conn, payer, r, BigInt(u.transactionIndex));
  if (!exec) {
    console.log(`re-run: npx tsx migrate.ts execute-upgrade ${program.toBase58()}`);
    return;
  }
  const rec = readRecord();
  rec.upgrades!.find((x) => x.transactionIndex === u.transactionIndex)!.executeSignature = exec;
  writeRecord(rec);
  console.log(`upgrade #${u.transactionIndex} executed: ${exec}`);
}

async function prove(conn: Connection, payer: Keypair): Promise<void> {
  const r = readRecord();
  const vault = new PublicKey(r.vault);
  const pending = r.proofs.find((p) => !p.executeSignature);
  let index: bigint;
  if (pending) {
    index = BigInt(pending.transactionIndex);
  } else {
    const done = await proposeAndApprove(conn, payer, r, [SystemProgram.transfer({ fromPubkey: vault, toPubkey: vault, lamports: 0 })]);
    index = done.index;
    r.proofs.push({ transactionIndex: index.toString(), proposeSignature: done.propose, approvals: done.approvals });
    writeRecord(r);
    console.log(`proposal #${index}: created, approved by both members`);
  }
  const exec = await executeWhenReady(conn, payer, r, index);
  if (!exec) {
    console.log("Re-run prove then to execute.");
    return;
  }
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
      const handedOver = r.treasury?.executeSignature && r.treasury.mint === mv.target ? r.treasury.signer : undefined;
      for (const [k, v] of Object.entries(await mintAuthorities(conn, mint))) {
        if (!custodied(k, v as string | null, r.vault, mint, handedOver)) problems.push(`${mv.target} ${k} authority is ${v}`);
      }
    }
  }
  if (r.treasury?.executeSignature) problems.push(...(await treasuryProblems(conn, r.treasury)));
  return problems;
}

const TREASURY_PROGRAM = new PublicKey(process.env.TREASURY_PROGRAM ?? "4BszRuAP4Gp7vdyu9MjE28gV8YTNtJZSRJRMQHdMut1y");
const anchorDisc = (name: string) => createHash("sha256").update(`global:${name}`).digest().subarray(0, 8);

/** The NPCS metadata `status` once the backing is the program's, not a process. */
export const treasuryStatus = (program: PublicKey) =>
  `IN FORCE: USDC 1:1 backing enforced on-chain by npcs-treasury ${program.toBase58()} (mint only on deposit; I1 supply == vault USDC checked at every mint and burn); ` +
  "thaw/freeze through Token ACL to credit-gate; permanent-delegate and metadata authority = Squads 2-of-2 vault";

export function treasuryPdas(program: PublicKey, mint: PublicKey) {
  const [treasury] = PublicKey.findProgramAddressSync([Buffer.from("treasury"), mint.toBuffer()], program);
  const [signer] = PublicKey.findProgramAddressSync([Buffer.from("treasury_signer"), treasury.toBuffer()], program);
  const [vault] = PublicKey.findProgramAddressSync([Buffer.from("vault"), treasury.toBuffer()], program);
  return { treasury, signer, vault };
}

/**
 * #148: hand the NPCS mint authority to npcs-treasury in ONE vault
 * transaction, so there is no moment in which anyone could call the
 * treasury's `initialize` (which makes its caller the treasury authority)
 * or in which I1 does not hold:
 *   1. Token-2022 SetAuthority(MintTokens): vault → the treasury signer PDA;
 *   2. npcs-treasury `initialize`, authority (and rent payer) = the vault;
 *   3. the backing USDC, supply / scale, sleeve → the treasury vault;
 *   4. the metadata `status` field says the backing is now enforced
 *      (the vault first tops the mint up for the longer value's rent).
 */
async function proposeTreasury(conn: Connection, payer: Keypair, mint: PublicKey, sleeve: PublicKey, program = TREASURY_PROGRAM): Promise<void> {
  const r = readRecord();
  if (r.treasury && !r.treasury.executeSignature) throw new Error(`treasury hand-over #${r.treasury.transactionIndex} is pending: execute-treasury`);
  if (r.treasury?.executeSignature) throw new Error(`the mint authority already went to ${r.treasury.signer}`);
  const vault = new PublicKey(r.vault);
  const prog = await conn.getAccountInfo(program);
  if (!prog?.executable) throw new Error(`npcs-treasury ${program} is not deployed on ${CLUSTER}`);
  const m = await getMint(conn, mint, "confirmed", TOKEN_2022_PROGRAM_ID);
  if (m.mintAuthority?.toBase58() !== r.vault) throw new Error(`mint authority is ${m.mintAuthority}, not the vault`);
  const sleeveInfo = await conn.getAccountInfo(sleeve);
  if (!sleeveInfo) throw new Error(`sleeve ${sleeve} not found`);
  const usdcTp = sleeveInfo.owner;
  const sl = await getAccount(conn, sleeve, "confirmed", usdcTp);
  if (sl.owner.toBase58() !== r.vault) throw new Error(`sleeve ${sleeve} is owned by ${sl.owner}, not the vault`);
  const usdc = await getMint(conn, sl.mint, "confirmed", usdcTp);
  if (m.decimals < usdc.decimals) throw new Error("asset decimals below USDC's");
  const scale = 10n ** BigInt(m.decimals - usdc.decimals);
  if (m.supply % scale !== 0n) throw new Error(`supply ${m.supply} is not a whole number of USDC units`);
  const amount = m.supply / scale;
  if (sl.amount < amount) throw new Error(`sleeve holds ${sl.amount} < ${amount} needed to back supply ${m.supply}: I1 would not hold`);
  const { treasury, signer, vault: tvault } = treasuryPdas(program, mint);

  // Metadata: the new status, and the rent the longer value needs.
  const meta = await getTokenMetadata(conn, mint, "confirmed", TOKEN_2022_PROGRAM_ID);
  const status = treasuryStatus(program);
  const old = meta?.additionalMetadata.find(([k]) => k === "status")?.[1];
  const ixs: TransactionInstruction[] = [
    createSetAuthorityInstruction(mint, vault, AuthorityType.MintTokens, signer, [], TOKEN_2022_PROGRAM_ID),
    new TransactionInstruction({
      programId: program,
      keys: [
        { pubkey: treasury, isSigner: false, isWritable: true },
        { pubkey: tvault, isSigner: false, isWritable: true },
        { pubkey: signer, isSigner: false, isWritable: false },
        { pubkey: mint, isSigner: false, isWritable: false },
        { pubkey: sl.mint, isSigner: false, isWritable: false },
        { pubkey: vault, isSigner: true, isWritable: true },
        { pubkey: usdcTp, isSigner: false, isWritable: false },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data: anchorDisc("initialize"),
    }),
    createTransferCheckedInstruction(sleeve, sl.mint, tvault, vault, amount, usdc.decimals, [], usdcTp),
  ];
  let topUp = 0;
  if (meta) {
    const mintAcc = (await conn.getAccountInfo(mint))!;
    // Borsh (u32 len + bytes) per string: a new entry adds key + value, an update the length difference.
    const grow = old === undefined ? 8 + Buffer.byteLength("status") + Buffer.byteLength(status) : Buffer.byteLength(status) - Buffer.byteLength(old);
    const need = await conn.getMinimumBalanceForRentExemption(mintAcc.data.length + Math.max(grow, 0));
    topUp = Math.max(0, need - mintAcc.lamports);
    if (topUp > 0) ixs.push(SystemProgram.transfer({ fromPubkey: vault, toPubkey: mint, lamports: topUp }));
    ixs.push(createUpdateFieldInstruction({ programId: TOKEN_2022_PROGRAM_ID, metadata: mint, updateAuthority: vault, field: "status", value: status }));
  }
  // The vault pays the treasury + vault-account rent and the top-up inside the vault transaction.
  const rent = (await conn.getMinimumBalanceForRentExemption(8 + 32 * 4 + 8 + 8 + 1 + 1 + 1)) + (await conn.getMinimumBalanceForRentExemption(165)) + topUp;
  const have = await conn.getBalance(vault);
  if (have < rent + 1_000_000) await send(conn, [SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: vault, lamports: rent + 1_000_000 - have })], [payer]);

  const { index, propose, approvals } = await proposeAndApprove(conn, payer, r, ixs);
  r.treasury = {
    program: program.toBase58(), mint: mint.toBase58(), treasury: treasury.toBase58(), signer: signer.toBase58(), vault: tvault.toBase58(),
    usdcMint: sl.mint.toBase58(), sleeve: sleeve.toBase58(), amount: amount.toString(), status,
    transactionIndex: index.toString(), proposeSignature: propose, approvals,
  };
  writeRecord(r);
  console.log(`treasury hand-over proposal #${index}: mint ${mint} → npcs-treasury ${treasury} (signer ${signer}), ${amount} USDC sleeve → ${tvault}: created, approved by both members`);
  const sig = await executeWhenReady(conn, payer, r, index);
  if (sig) await finishTreasury(conn, r, sig);
}

async function treasuryProblems(conn: Connection, t: TreasuryHandover): Promise<string[]> {
  const problems: string[] = [];
  const mint = new PublicKey(t.mint);
  const m = await getMint(conn, mint, "confirmed", TOKEN_2022_PROGRAM_ID);
  if (m.mintAuthority?.toBase58() !== t.signer) problems.push(`mint authority is ${m.mintAuthority}, not the treasury signer ${t.signer}`);
  const acct = await conn.getAccountInfo(new PublicKey(t.treasury));
  if (!acct || acct.owner.toBase58() !== t.program) problems.push(`treasury ${t.treasury} missing or not owned by ${t.program}`);
  // Treasury: disc(8) | authority — redeem power must sit with the vault, not whoever called initialize.
  else if (new PublicKey(acct.data.subarray(8, 40)).toBase58() !== readRecord().vault) {
    problems.push(`treasury authority is ${new PublicKey(acct.data.subarray(8, 40))}, not the vault`);
  }
  const vInfo = await conn.getAccountInfo(new PublicKey(t.vault));
  if (!vInfo) problems.push(`treasury vault ${t.vault} missing`);
  else {
    const v = await getAccount(conn, new PublicKey(t.vault), "confirmed", vInfo.owner);
    const usdc = await getMint(conn, new PublicKey(t.usdcMint), "confirmed", vInfo.owner);
    const scale = 10n ** BigInt(m.decimals - usdc.decimals);
    if (v.amount * scale !== m.supply) problems.push(`I1 broken: supply ${m.supply} != vault ${v.amount} × ${scale}`);
  }
  return problems;
}

async function finishTreasury(conn: Connection, r: Record, sig: string): Promise<void> {
  r.treasury!.executeSignature = sig;
  writeRecord(r);
  const problems = await treasuryProblems(conn, r.treasury!);
  if (problems.length) throw new Error(`hand-over executed (${sig}) but:\n  ${problems.join("\n  ")}`);
  console.log(`treasury hand-over executed: ${sig}\nmint authority = ${r.treasury!.signer} (npcs-treasury); I1 holds`);
}

async function executeTreasury(conn: Connection, payer: Keypair): Promise<void> {
  const r = readRecord();
  if (!r.treasury) throw new Error("no treasury hand-over proposed");
  if (r.treasury.executeSignature) return console.log(`already executed: ${r.treasury.executeSignature}`);
  const sig = await executeWhenReady(conn, payer, r, BigInt(r.treasury.transactionIndex));
  if (sig) await finishTreasury(conn, r, sig);
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
    case "propose-upgrade": {
      const p = payer();
      return proposeUpgrade(conn, p, new PublicKey(need(arg, "<program>")), new PublicKey(need(flag("buffer"), "--buffer")),
        new PublicKey(flag("spill") ?? p.publicKey));
    }
    case "execute-upgrade":
      return executeUpgrade(conn, payer(), new PublicKey(need(arg, "<program>")));
    case "propose-treasury":
      return proposeTreasury(conn, payer(), new PublicKey(need(arg, "<mint>")), new PublicKey(need(flag("sleeve"), "--sleeve")));
    case "execute-treasury":
      return executeTreasury(conn, payer());
    case "verify": {
      const problems = await verify(conn, readRecord());
      if (problems.length) throw new Error(`verify FAILED:\n  ${problems.join("\n  ")}`);
      const r = readRecord();
      console.log(`verify OK — ${r.multisig} is 2-of-2 (timelock ${r.timeLockSeconds}s); ${r.moves.length} authority move(s) hold`);
      return;
    }
    default:
      throw new Error("usage: migrate.ts <create|move-program|move-mint|prove|propose-upgrade|execute-upgrade|propose-treasury|execute-treasury|verify> — see the header");
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
