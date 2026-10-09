/**
 * Put a mint behind credit-gate through Token ACL (CG-P1, #122).
 *
 *   tsx wire.ts root    --upgrade-authority <kp> [--root <pubkey>]      # GateRoot (#93), once
 *   tsx wire.ts market  --authority <kp> --mint <mint> [--holder-cap 8]  # config + policy +
 *                       [--jurisdictions US,GB] [--min-kyc 1]            #   block-list + Token
 *                       [--accreditation true]                           #   ACL extra metas
 *   tsx wire.ts acl     --freeze-authority <kp> --mint <mint>            # MintConfig: freeze
 *                                                                        #   authority → Token ACL,
 *                                                                        #   gate = credit-gate
 *   tsx wire.ts validate <wallet> --mint <mint>                         # anyone pays; CG-P1
 *   tsx wire.ts thaw    <token-account> --mint <mint>                   # anyone: Token ACL thaw
 *   tsx wire.ts append-lot <wallet> --authority <kp> --mint <mint>      # CG-P3 primary-mint lot
 *                       --qty <base units>                               #   (gate-config authority)
 *   tsx wire.ts register-custodian <custodian> --authority <kp>         # LF-P5: a wallet that
 *                       --mint <mint>                                    #   holds lots for others
 *                                                                        #   (facility signer PDA)
 *   tsx wire.ts verify  --mint <mint>                                   # keyless
 *
 * The SAS side (credential, schemas, attester) comes from `sas.ts`'s
 * `deployment-<cluster>.json`. Env: CLUSTER, RPC_URL, PAYER, GATE_PROGRAM
 * (default the registry id), SAS_RECORD (default ./deployment-<cluster>.json).
 * Instruction encodings match target/idl/credit_gate.json and are exercised
 * end to end by test-localnet.sh against the real Token ACL and SAS.
 */
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { attestationPda } from "./sas.ts";

const CLUSTER = process.env.CLUSTER ?? "devnet";
const RPC = process.env.RPC_URL ?? (CLUSTER === "devnet" ? "https://api.devnet.solana.com" : "http://127.0.0.1:8899");
export const GATE = new PublicKey(process.env.GATE_PROGRAM ?? "5ycYLKQjrbwxQ6VQLUSkQZXwLMn1zEeJ6f5ogFYzX6XX");
export const TOKEN_ACL = new PublicKey("TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP");
export const TOKEN_2022 = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const LOADER = new PublicKey("BPFLoaderUpgradeab1e11111111111111111111111");
const SAS_RECORD = process.env.SAS_RECORD ?? join(import.meta.dirname, `deployment-${CLUSTER}.json`);

const disc = (name: string) => createHash("sha256").update(`global:${name}`).digest().subarray(0, 8);
const pda = (seeds: (Buffer | Uint8Array | string)[], program = GATE) =>
  PublicKey.findProgramAddressSync(seeds.map((s) => (typeof s === "string" ? Buffer.from(s) : Buffer.from(s))), program)[0];
const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const i64 = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigInt64LE(n); return b; };

export const seeds = {
  root: () => pda(["gate_root"]),
  config: (m: PublicKey) => pda(["config", m.toBuffer()]),
  policy: (m: PublicKey) => pda(["policy", m.toBuffer()]),
  blocklist: (m: PublicKey) => pda(["blocklist", m.toBuffer()]),
  blocked: (m: PublicKey, w: PublicKey) => pda(["blocked", m.toBuffer(), w.toBuffer()]),
  holder: (m: PublicKey, w: PublicKey) => pda(["holder", m.toBuffer(), w.toBuffer()]),
  investor: (m: PublicKey, id: Buffer) => pda(["investor", m.toBuffer(), id]),
  lots: (m: PublicKey, w: PublicKey) => pda(["lots", m.toBuffer(), w.toBuffer()]),
  thawMetas: (m: PublicKey) => pda(["thaw_extra_account_metas", m.toBuffer()]),
  freezeMetas: (m: PublicKey) => pda(["freeze_extra_account_metas", m.toBuffer()]),
  mintConfig: (m: PublicKey) => pda(["MINT_CONFIG", m.toBuffer()], TOKEN_ACL),
  flag: (ta: PublicKey) => pda(["FLAG_ACCOUNT", ta.toBuffer()], TOKEN_ACL),
};

const meta = (pubkey: PublicKey, isWritable = false, isSigner = false) => ({ pubkey, isWritable, isSigner });
const ix = (programId: PublicKey, keys: ReturnType<typeof meta>[], data: Buffer) =>
  new TransactionInstruction({ programId, keys, data });

function loadKeypair(path: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(path.replace(/^~(?=\/)/, homedir()), "utf8"))));
}
const sas = () => JSON.parse(readFileSync(SAS_RECORD, "utf8"));
const send = (c: Connection, ixs: TransactionInstruction[], signers: Keypair[]) =>
  sendAndConfirmTransaction(c, new Transaction().add(...ixs), signers, { commitment: "confirmed" });

export function rootIx(upgrade: PublicKey, root: PublicKey) {
  return ix(GATE, [
    meta(seeds.root(), true), meta(upgrade, true, true), meta(GATE),
    meta(pda([GATE.toBuffer()], LOADER)), meta(SystemProgram.programId),
  ], Buffer.concat([disc("initialize_root"), root.toBuffer()]));
}

export function marketIxs(o: {
  authority: PublicKey; mint: PublicKey; attester: PublicKey; identitySchema: PublicKey;
  accreditationSchema: PublicKey; credential: PublicKey; holderCap: number; seasoning: bigint;
  horizon: bigint; jurisdictions: string[]; minKyc: number; requireAccreditation: boolean;
}) {
  const m = o.mint;
  const config = ix(GATE, [
    meta(seeds.config(m), true), meta(seeds.root()), meta(m), meta(o.authority, true, true), meta(SystemProgram.programId),
  ], Buffer.concat([
    disc("initialize_config"), u32(1), u32(o.holderCap), i64(o.seasoning), i64(o.horizon),
    u32(1), o.attester.toBuffer(), o.identitySchema.toBuffer(), o.accreditationSchema.toBuffer(),
  ]));
  const policy = ix(GATE, [
    meta(seeds.config(m)), meta(seeds.policy(m), true), meta(o.identitySchema), meta(o.accreditationSchema),
    meta(m), meta(o.authority, true, true), meta(SystemProgram.programId),
  ], Buffer.concat([
    disc("initialize_policy"), o.credential.toBuffer(), u32(o.jurisdictions.length),
    ...o.jurisdictions.map((j) => Buffer.from(j, "ascii")), Buffer.from([o.minKyc, o.requireAccreditation ? 1 : 0]),
  ]));
  const blocklist = ix(GATE, [
    meta(seeds.config(m), true), meta(seeds.blocklist(m), true), meta(m), meta(o.authority, true, true), meta(SystemProgram.programId),
  ], disc("initialize_blocklist"));
  const metas = ix(GATE, [
    meta(seeds.config(m)), meta(seeds.thawMetas(m), true), meta(seeds.freezeMetas(m), true), meta(m),
    meta(o.authority, true, true), meta(SystemProgram.programId),
  ], disc("initialize_token_acl_metas"));
  return [config, policy, blocklist, metas];
}

/** Token ACL CreateConfig (0) + TogglePermissionlessInstructions (8: freeze, thaw on). */
export function aclIxs(freezeAuthority: PublicKey, payer: PublicKey, mint: PublicKey) {
  const mc = seeds.mintConfig(mint);
  return [
    ix(TOKEN_ACL, [
      meta(payer, true, true), meta(freezeAuthority, false, true), meta(mint, true), meta(mc, true),
      meta(SystemProgram.programId), meta(TOKEN_2022),
    ], Buffer.concat([Buffer.from([0]), GATE.toBuffer()])),
    ix(TOKEN_ACL, [meta(freezeAuthority, false, true), meta(mc, true)], Buffer.from([8, 1, 1])),
  ];
}

export function validateIx(payer: PublicKey, mint: PublicKey, wallet: PublicKey, investorId: Buffer) {
  const r = sas();
  const credential = new PublicKey(r.credential);
  return ix(GATE, [
    meta(seeds.config(mint)), meta(seeds.policy(mint), true), meta(seeds.blocked(mint, wallet)),
    meta(attestationPda(credential, new PublicKey(r.schemas["nundina.identity.v1"]), wallet)),
    meta(attestationPda(credential, new PublicKey(r.schemas["nundina.accreditation.v1"]), wallet)),
    meta(seeds.holder(mint, wallet), true), meta(seeds.investor(mint, investorId), true),
    meta(seeds.lots(mint, wallet), true), meta(mint), meta(payer, true, true), meta(SystemProgram.programId),
  ], Buffer.concat([disc("validate"), wallet.toBuffer()]));
}

/** CG-P3 `append_lot`: a primary-mint lot (source PrimaryMint, exemption ISSUANCE = 3). */
export function appendLotIx(authority: PublicKey, mint: PublicKey, wallet: PublicKey, qty: bigint) {
  const q = Buffer.alloc(8); q.writeBigUInt64LE(qty);
  return ix(GATE, [
    meta(seeds.config(mint)), meta(seeds.lots(mint, wallet), true), meta(seeds.blocked(mint, wallet)),
    meta(mint), meta(authority, false, true),
  ], Buffer.concat([disc("append_lot"), q, Buffer.from([0, 0, 3])]));
}

/** LF-P5: register `custodian` (liquidity-facility's signer PDA) on the market. */
export function registerCustodianIx(authority: PublicKey, mint: PublicKey, custodian: PublicKey) {
  return ix(GATE, [
    meta(seeds.config(mint)), meta(pda(["custodian", mint.toBuffer(), custodian.toBuffer()]), true),
    meta(mint), meta(authority, true, true), meta(SystemProgram.programId),
  ], Buffer.concat([disc("register_custodian"), custodian.toBuffer()]));
}

/** Token ACL ThawPermissionless (6); the gate's extras in its TLV order. */
export async function thawIx(c: Connection, payer: PublicKey, mint: PublicKey, tokenAccount: PublicKey) {
  const ta = await c.getAccountInfo(tokenAccount);
  if (!ta) throw new Error("token account not found");
  const owner = new PublicKey(ta.data.subarray(32, 64));
  const link = await c.getAccountInfo(seeds.holder(mint, owner));
  if (!link) throw new Error(`${owner.toBase58()} has no holder link: run validate first`);
  return ix(TOKEN_ACL, [
    meta(payer, true, true), meta(mint), meta(tokenAccount, true), meta(seeds.flag(tokenAccount), true),
    meta(owner), meta(seeds.mintConfig(mint)), meta(TOKEN_2022), meta(SystemProgram.programId), meta(GATE),
    meta(seeds.thawMetas(mint)), meta(seeds.config(mint)), meta(seeds.policy(mint)),
    meta(seeds.blocked(mint, owner)), meta(seeds.holder(mint, owner)),
    meta(new PublicKey(link.data.subarray(72, 104))), meta(new PublicKey(link.data.subarray(104, 136))),
  ], Buffer.from([6]));
}

/** Keyless: is `mint` behind credit-gate, and is the gate set up for it? */
export async function verify(c: Connection, mint: PublicKey): Promise<string[]> {
  const p: string[] = [];
  const need = async (k: PublicKey, owner: PublicKey, what: string) => {
    const a = await c.getAccountInfo(k);
    if (!a || !a.owner.equals(owner)) p.push(`${what} ${k.toBase58()} missing or not owned by ${owner.toBase58()}`);
    return a;
  };
  await need(seeds.config(mint), GATE, "gate config");
  await need(seeds.policy(mint), GATE, "gate policy");
  await need(seeds.blocklist(mint), GATE, "block-list header");
  for (const [k, d, what] of [
    [seeds.thawMetas(mint), [8, 175, 169, 129, 137, 74, 61, 241], "thaw extra metas"],
    [seeds.freezeMetas(mint), [214, 141, 109, 75, 248, 1, 45, 29], "freeze extra metas"],
  ] as const) {
    const a = await need(k, GATE, what);
    if (a && !Buffer.from(d).equals(a.data.subarray(0, 8))) p.push(`${what}: wrong TLV discriminator`);
  }
  const mc = await need(seeds.mintConfig(mint), TOKEN_ACL, "Token ACL MintConfig");
  if (mc) {
    if (!new PublicKey(mc.data.subarray(68, 100)).equals(GATE)) p.push("MintConfig gating program is not credit-gate");
    if (mc.data[2] !== 1) p.push("permissionless thaw disabled");
    if (mc.data[3] !== 1) p.push("permissionless freeze disabled");
  }
  const m = await c.getAccountInfo(mint);
  // Token-2022 mint: freeze authority COption at bytes 46..82.
  if (!m || m.data.readUInt32LE(46) !== 1 || !new PublicKey(m.data.subarray(50, 82)).equals(seeds.mintConfig(mint))) {
    p.push("mint freeze authority is not the Token ACL MintConfig");
  }
  return p;
}

async function main() {
  const [cmd, arg] = process.argv.slice(2);
  const o: Record<string, string> = {};
  for (let i = 2; i < process.argv.length; i++) {
    const m = process.argv[i]!.match(/^--(.+)$/);
    if (m) o[m[1]!] = process.argv[++i]!;
  }
  const c = new Connection(RPC, "confirmed");
  const payer = () => loadKeypair(process.env.PAYER ?? "~/.config/solana/id.json");
  const mint = () => new PublicKey(o.mint ?? (() => { throw new Error("--mint is required"); })());
  switch (cmd) {
    case "root": {
      const up = loadKeypair(o["upgrade-authority"]!);
      console.log(await send(c, [rootIx(up.publicKey, new PublicKey(o.root ?? up.publicKey))], [up]));
      return;
    }
    case "market": {
      const r = sas();
      const auth = loadKeypair(o.authority!);
      const ixs = marketIxs({
        authority: auth.publicKey, mint: mint(), attester: new PublicKey(r.authorizedSigners[0]),
        identitySchema: new PublicKey(r.schemas["nundina.identity.v1"]),
        accreditationSchema: new PublicKey(r.schemas["nundina.accreditation.v1"]),
        credential: new PublicKey(r.credential), holderCap: Number(o["holder-cap"] ?? 8),
        seasoning: BigInt(o.seasoning ?? 604_800), horizon: BigInt(o.horizon ?? 86_400),
        jurisdictions: (o.jurisdictions ?? "US").split(","), minKyc: Number(o["min-kyc"] ?? 1),
        requireAccreditation: (o.accreditation ?? "true") !== "false",
      });
      console.log(await send(c, ixs.slice(0, 1), [auth]));
      console.log(await send(c, ixs.slice(1), [auth]));
      return;
    }
    case "acl": {
      const fa = loadKeypair(o["freeze-authority"]!);
      console.log(await send(c, aclIxs(fa.publicKey, fa.publicKey, mint()), [fa]));
      return;
    }
    case "validate": {
      const wallet = new PublicKey(arg!);
      const r = sas();
      const id = await c.getAccountInfo(attestationPda(new PublicKey(r.credential), new PublicKey(r.schemas["nundina.identity.v1"]), wallet));
      if (!id) throw new Error("no identity attestation for this wallet (sas.ts attest)");
      // data starts after disc + 3 keys + u32 len: the first 16 bytes are investor_id.
      const investor = Buffer.from(id.data.subarray(101, 117));
      const p = payer();
      const sig = await send(c, [validateIx(p.publicKey, mint(), wallet, investor)], [p]);
      const tx = await c.getTransaction(sig, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
      const decision = (tx?.meta?.logMessages ?? []).find((l) => l.startsWith("Program data: "));
      console.log(`${sig}\n${decision ?? ""}`);
      return;
    }
    case "thaw": {
      const p = payer();
      console.log(await send(c, [await thawIx(c, p.publicKey, mint(), new PublicKey(arg!))], [p]));
      return;
    }
    case "append-lot": {
      const auth = loadKeypair(o.authority!);
      const qty = BigInt(o.qty ?? (() => { throw new Error("--qty is required"); })());
      console.log(await send(c, [appendLotIx(auth.publicKey, mint(), new PublicKey(arg!), qty)], [auth]));
      return;
    }
    case "register-custodian": {
      const auth = loadKeypair(o.authority!);
      console.log(await send(c, [registerCustodianIx(auth.publicKey, mint(), new PublicKey(arg!))], [auth]));
      return;
    }
    case "verify": {
      const problems = await verify(c, mint());
      if (problems.length) throw new Error(`verify FAILED:\n  ${problems.join("\n  ")}`);
      console.log(`verify OK — ${mint().toBase58()} thaws only through credit-gate ${GATE.toBase58()}`);
      return;
    }
    default:
      throw new Error("usage: wire.ts <root|market|acl|validate|thaw|append-lot|register-custodian|verify> — see the header");
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    console.error(e instanceof Error ? e.message : e);
    if ((e as { logs?: string[] }).logs) console.error((e as { logs: string[] }).logs.join("\n"));
    process.exit(1);
  });
}
