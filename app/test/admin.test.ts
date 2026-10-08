/** Admin instructions pinned against the IDLs: account order, writability, signers, arg bytes. */
import { describe, expect, it } from "vitest";
import { AccountRole, address, getAddressEncoder, type Address, type Instruction } from "@solana/kit";
import railsIdl from "@/idl/payment_rails.json";
import markIdl from "@/idl/mark_engine.json";
import gateIdl from "@/idl/credit_gate.json";
import { adminIx, proposalJson } from "@/lib/admin-tx";
import { programDataAuthority } from "@/lib/admin";
import { pdas, SYSTEM_PROGRAM } from "@/lib/programs";

type IdlIx = { name: string; discriminator: number[]; accounts: { name: string; writable?: boolean; signer?: boolean }[] };
const find = (idl: { instructions: IdlIx[] }, n: string) => idl.instructions.find((i) => i.name === n)!;

const auth = address("9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ");
const mint = address("6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4");
const wallet = address("BJL7L58MX82DiGxtjcq5VbzWuLLZe26U6R8PCfdJ2EMh");
const engine = address("SysvarC1ock11111111111111111111111111111111");
const h = new Uint8Array(32).fill(7);

function pinned(ix: Instruction, idl: IdlIx, expected: Record<string, Address>, args: number[]) {
  expect(ix.accounts!.map((a) => a.address)).toEqual(idl.accounts.map((a) => expected[a.name]));
  ix.accounts!.forEach((a, i) => {
    const m = idl.accounts[i]!;
    const w = a.role === AccountRole.WRITABLE || a.role === AccountRole.WRITABLE_SIGNER;
    const s = a.role === AccountRole.READONLY_SIGNER || a.role === AccountRole.WRITABLE_SIGNER;
    expect([m.name, w, s]).toEqual([m.name, !!m.writable, !!m.signer]);
  });
  expect([...ix.data!]).toEqual([...idl.discriminator, ...args]);
}

describe("admin instructions", () => {
  it("payment-rails set_breaker / set_fees / grant_engine", async () => {
    const config = await pdas.railsConfig();
    pinned(await adminIx.setBreaker(auth, "Depeg", true, h), find(railsIdl as never, "set_breaker"), { config, authority: auth }, [1, 1, ...h]);
    pinned(await adminIx.setFees(auth, 0, 5), find(railsIdl as never, "set_fees"), { config, authority: auth }, [0, 0, 5, 0]);
    pinned(
      await adminIx.grantEngine(auth, engine, 1),
      find(railsIdl as never, "grant_engine"),
      { config, grant: await pdas.engineGrant(engine), authority: auth, system_program: SYSTEM_PROGRAM },
      [...getAddressEncoder().encode(engine), 1],
    );
  });

  it("mark-engine suspend / resume", async () => {
    const feed = await pdas.feed(mint);
    pinned(await adminIx.suspendFeed(auth, mint, h), find(markIdl as never, "suspend"), { feed, authority: auth }, [...h]);
    pinned(await adminIx.resumeFeed(auth, mint), find(markIdl as never, "resume"), { feed, authority: auth }, []);
  });

  it("credit-gate block_wallet / unblock_wallet", async () => {
    const e = {
      config: await pdas.gateConfig(mint),
      blocklist: await pdas.blocklist(mint),
      entry: await pdas.blocked(mint, wallet),
      mint,
      gate_config_authority: auth,
      system_program: SYSTEM_PROGRAM,
    };
    const w = [...getAddressEncoder().encode(wallet)];
    pinned(await adminIx.blockWallet(auth, mint, wallet, h), find(gateIdl as never, "block_wallet"), e, [...w, ...h]);
    pinned(await adminIx.unblockWallet(auth, mint, wallet), find(gateIdl as never, "unblock_wallet"), e, w);
  });

  it("exports a vault proposal with every account's role", async () => {
    const j = JSON.parse(proposalJson(await adminIx.setFees(auth, 0, 5)));
    expect(j.accounts.map((a: { role: string }) => a.role)).toEqual(["writable", "signer"]);
    expect(j.programId).toBe("GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn");
  });
});

describe("ProgramData", () => {
  it("reads the slot and the upgrade authority (or none)", () => {
    const d = new Uint8Array(45 + 8);
    d[0] = 3;
    new DataView(d.buffer).setBigUint64(4, 508_579_189n, true);
    d[12] = 1;
    d.set(getAddressEncoder().encode(auth), 13);
    expect(programDataAuthority(d)).toEqual({ slot: 508_579_189n, authority: auth });
    d[12] = 0;
    expect(programDataAuthority(d).authority).toBeNull();
    d[0] = 2;
    expect(() => programDataAuthority(d)).toThrow();
  });
});
