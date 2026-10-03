/**
 * NUNDINA on the Solana Attestation Service (SRS 08 §5, #122).
 *
 *   tsx sas.ts setup                         # credential + the three schemas (idempotent)
 *   tsx sas.ts attest <wallet> --investor <uuid> --jurisdiction US --kyc 2
 *                     [--accredited true --method 1] [--days 90]
 *                                            # registrar step: identity + accreditation
 *   tsx sas.ts verify                        # keyless: credential, signers, schema layouts
 *
 * Env: CLUSTER (devnet), RPC_URL, PAYER (fee payer, default
 * ~/.config/solana/id.json), KEYS_DIR (credential authority + attester
 * keypairs; default ~/.config/nundina/sas-<cluster>, never in the repo).
 *
 * Byte encodings follow SAS's own client (solana-attestation-service-client
 * 1.0.9) and were exercised against the real program in
 * programs/credit-gate/tests-svm/tests/sas_token_acl.rs.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
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

export const SAS = new PublicKey("22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG");
const CLUSTER = process.env.CLUSTER ?? "devnet";
const RPC = process.env.RPC_URL ?? (CLUSTER === "devnet" ? "https://api.devnet.solana.com" : "http://127.0.0.1:8899");
const KEYS = process.env.KEYS_DIR ?? join(homedir(), ".config", "nundina", `sas-${CLUSTER}`);
const RECORD = join(import.meta.dirname, `deployment-${CLUSTER}.json`);
export const CREDENTIAL_NAME = "nundina";

/** SAS schema field types (program `state/schema.rs`). */
const T = { U8: 0, U32: 2, U64: 3, U128: 4, I64: 8, Bool: 10, String: 12, VecU8: 13 } as const;

/** SRS 08 §5. Names, layouts and field names are part of the gate's interface. */
export const SCHEMAS = [
  {
    name: "nundina.identity.v1",
    description: "NUNDINA identity: investor_id (UUIDv7 as u128), ISO 3166-1 alpha-2 jurisdiction, KYC level",
    layout: [T.U128, T.String, T.U8],
    fields: ["investor_id", "jurisdiction", "kyc_level"],
  },
  {
    name: "nundina.accreditation.v1",
    description: "NUNDINA accreditation: accredited flag and verification method (expiry <= 90 days)",
    layout: [T.Bool, T.U8],
    fields: ["accredited", "method"],
  },
  {
    name: "nundina.redemption_request.v1",
    description: "NUNDINA redemption request: asset mint, qty, window date, queue rank",
    layout: [T.VecU8, T.U64, T.I64, T.U32],
    fields: ["asset", "qty", "window_date", "rank"],
  },
] as const;

const str = (s: string | Uint8Array) => {
  const b = typeof s === "string" ? Buffer.from(s) : Buffer.from(s);
  const len = Buffer.alloc(4);
  len.writeUInt32LE(b.length);
  return Buffer.concat([len, b]);
};
const u32 = (n: number) => {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
};

export const credentialPda = (authority: PublicKey) =>
  PublicKey.findProgramAddressSync([Buffer.from("credential"), authority.toBuffer(), Buffer.from(CREDENTIAL_NAME)], SAS)[0];
export const schemaPda = (credential: PublicKey, name: string) =>
  PublicKey.findProgramAddressSync([Buffer.from("schema"), credential.toBuffer(), Buffer.from(name), Buffer.from([1])], SAS)[0];
export const attestationPda = (credential: PublicKey, schema: PublicKey, nonce: PublicKey) =>
  PublicKey.findProgramAddressSync([Buffer.from("attestation"), credential.toBuffer(), schema.toBuffer(), nonce.toBuffer()], SAS)[0];

function keypair(path: string, create: boolean): Keypair {
  if (existsSync(path)) return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(path, "utf8"))));
  if (!create) throw new Error(`missing ${path}`);
  mkdirSync(KEYS, { recursive: true, mode: 0o700 });
  const k = Keypair.generate();
  writeFileSync(path, JSON.stringify(Array.from(k.secretKey)), { mode: 0o600 });
  return k;
}
const loadPath = (p: string) => keypair(p.replace(/^~(?=\/)/, homedir()), false);

async function send(conn: Connection, ix: TransactionInstruction, signers: Keypair[]) {
  return sendAndConfirmTransaction(conn, new Transaction().add(ix), signers, { commitment: "confirmed" });
}

async function setup(conn: Connection, payer: Keypair) {
  const authority = keypair(join(KEYS, "credential-authority.json"), true);
  const attester = keypair(join(KEYS, "attester.json"), true);
  const credential = credentialPda(authority.publicKey);
  const sigs: Record<string, string> = {};
  if (!(await conn.getAccountInfo(credential))) {
    sigs.credential = await send(
      conn,
      new TransactionInstruction({
        programId: SAS,
        keys: [
          { pubkey: payer.publicKey, isSigner: true, isWritable: true },
          { pubkey: credential, isSigner: false, isWritable: true },
          { pubkey: authority.publicKey, isSigner: true, isWritable: false },
          { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        ],
        data: Buffer.concat([Buffer.from([0]), str(CREDENTIAL_NAME), u32(1), attester.publicKey.toBuffer()]),
      }),
      [payer, authority],
    );
  }
  const schemas: Record<string, string> = {};
  for (const s of SCHEMAS) {
    const schema = schemaPda(credential, s.name);
    schemas[s.name] = schema.toBase58();
    if (await conn.getAccountInfo(schema)) continue;
    sigs[s.name] = await send(
      conn,
      new TransactionInstruction({
        programId: SAS,
        keys: [
          { pubkey: payer.publicKey, isSigner: true, isWritable: true },
          { pubkey: authority.publicKey, isSigner: true, isWritable: false },
          { pubkey: credential, isSigner: false, isWritable: false },
          { pubkey: schema, isSigner: false, isWritable: true },
          { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        ],
        data: Buffer.concat([
          Buffer.from([1]),
          str(s.name),
          str(s.description),
          str(Uint8Array.from(s.layout)),
          u32(s.fields.length),
          ...s.fields.map((f) => str(f)),
        ]),
      }),
      [payer, authority],
    );
  }
  const prior = existsSync(RECORD) ? JSON.parse(readFileSync(RECORD, "utf8")) : { signatures: {} };
  const record = {
    cluster: CLUSTER,
    sasProgram: SAS.toBase58(),
    credential: credential.toBase58(),
    credentialName: CREDENTIAL_NAME,
    credentialAuthority: authority.publicKey.toBase58(),
    authorizedSigners: [attester.publicKey.toBase58()],
    schemas,
    signatures: { ...prior.signatures, ...sigs },
  };
  writeFileSync(RECORD, JSON.stringify(record, null, 2) + "\n");
  console.log(JSON.stringify(record, null, 2));
}

/** Registrar step (SRS 08 §5): the attester issues identity + accreditation for `wallet`. */
async function attest(conn: Connection, payer: Keypair, wallet: PublicKey, o: Record<string, string>) {
  const r = JSON.parse(readFileSync(RECORD, "utf8"));
  const attester = keypair(join(KEYS, "attester.json"), false);
  const credential = new PublicKey(r.credential);
  const uuid = (o.investor ?? "").replace(/-/g, "");
  if (!/^[0-9a-f]{32}$/i.test(uuid)) throw new Error("--investor must be a UUID");
  const jurisdiction = o.jurisdiction ?? "";
  if (!/^[A-Z]{2}$/.test(jurisdiction)) throw new Error("--jurisdiction must be ISO 3166-1 alpha-2, upper case");
  const expiry = BigInt(Math.floor(Date.now() / 1000) + Number(o.days ?? 90) * 86_400);
  // u128 little-endian of the UUID's big-endian hex.
  const investor = Buffer.from(uuid, "hex").reverse();
  const issue = async (schemaName: string, data: Buffer) => {
    const schema = new PublicKey(r.schemas[schemaName]);
    const att = attestationPda(credential, schema, wallet);
    const exp = Buffer.alloc(8);
    exp.writeBigInt64LE(expiry);
    const sig = await send(
      conn,
      new TransactionInstruction({
        programId: SAS,
        keys: [
          { pubkey: payer.publicKey, isSigner: true, isWritable: true },
          { pubkey: attester.publicKey, isSigner: true, isWritable: false },
          { pubkey: credential, isSigner: false, isWritable: false },
          { pubkey: schema, isSigner: false, isWritable: false },
          { pubkey: att, isSigner: false, isWritable: true },
          { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
        ],
        data: Buffer.concat([Buffer.from([6]), wallet.toBuffer(), str(data), exp]),
      }),
      [payer, attester],
    );
    console.log(`${schemaName}: ${att.toBase58()} (tx ${sig})`);
  };
  await issue("nundina.identity.v1", Buffer.concat([investor, str(jurisdiction), Buffer.from([Number(o.kyc ?? 1)])]));
  await issue("nundina.accreditation.v1", Buffer.from([o.accredited === "false" ? 0 : 1, Number(o.method ?? 1)]));
}

/** Keyless check of the recorded credential and schemas against chain. */
export async function verify(conn: Connection): Promise<string[]> {
  const r = JSON.parse(readFileSync(RECORD, "utf8"));
  const problems: string[] = [];
  const cred = await conn.getAccountInfo(new PublicKey(r.credential));
  if (!cred || !cred.owner.equals(SAS) || cred.data[0] !== 0) problems.push("credential missing or not a SAS credential");
  else {
    const d = cred.data;
    if (!new PublicKey(d.subarray(1, 33)).equals(new PublicKey(r.credentialAuthority))) problems.push("credential authority differs");
    const nameLen = d.readUInt32LE(33);
    const off = 37 + nameLen;
    const n = d.readUInt32LE(off);
    const signers = Array.from({ length: n }, (_, i) => new PublicKey(d.subarray(off + 4 + 32 * i, off + 36 + 32 * i)).toBase58());
    if (signers.join() !== r.authorizedSigners.join()) problems.push(`authorized signers ${signers} ≠ recorded`);
  }
  for (const s of SCHEMAS) {
    const a = await conn.getAccountInfo(new PublicKey(r.schemas[s.name]));
    if (!a || !a.owner.equals(SAS) || a.data[0] !== 1) {
      problems.push(`${s.name}: missing or not a SAS schema`);
      continue;
    }
    // discriminator | credential | name | description | layout | field_names | paused | version
    let o = 33;
    const skip = () => {
      const l = a.data.readUInt32LE(o);
      const v = a.data.subarray(o + 4, o + 4 + l);
      o += 4 + l;
      return v;
    };
    if (!new PublicKey(a.data.subarray(1, 33)).equals(new PublicKey(r.credential))) problems.push(`${s.name}: wrong credential`);
    skip();
    skip();
    const layout = Array.from(skip());
    if (layout.join() !== s.layout.join()) problems.push(`${s.name}: layout ${layout} ≠ ${s.layout}`);
  }
  return problems;
}

async function main() {
  const [cmd, arg] = process.argv.slice(2);
  const opts: Record<string, string> = {};
  for (let i = 2; i < process.argv.length; i++) {
    const m = process.argv[i]!.match(/^--(.+)$/);
    if (m) opts[m[1]!] = process.argv[++i]!;
  }
  const conn = new Connection(RPC, "confirmed");
  const payer = () => loadPath(process.env.PAYER ?? "~/.config/solana/id.json");
  if (cmd === "setup") return setup(conn, payer());
  if (cmd === "attest") return attest(conn, payer(), new PublicKey(arg!), opts);
  if (cmd === "verify") {
    const p = await verify(conn);
    if (p.length) throw new Error(`verify FAILED:\n  ${p.join("\n  ")}`);
    console.log(`verify OK — credential and ${SCHEMAS.length} schemas match ${RECORD}`);
    return;
  }
  throw new Error("usage: sas.ts <setup|attest|verify> — see the header");
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    console.error(e instanceof Error ? e.message : e);
    if ((e as { logs?: string[] }).logs) console.error((e as { logs: string[] }).logs.join("\n"));
    process.exit(1);
  });
}
