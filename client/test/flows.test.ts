/**
 * The queue / facility / tranche / treasury writes (#202) are pinned
 * against the programs' own IDLs: every account sits at the IDL's position
 * with the IDL's writability and signer flag (PDAs the engines sign for by
 * CPI are de-signed), every PDA is the IDL's seeds, and the data is the
 * IDL discriminator followed by the arguments in IDL order.
 */
import { describe, expect, it } from "vitest";
import { AccountRole, address, getAddressEncoder, getProgramDerivedAddress, type Address, type Instruction } from "@solana/kit";
import queueIdl from "@/idl/queue_claim.json";
import facilityIdl from "@/idl/liquidity_facility.json";
import trancheIdl from "@/idl/tranche_engine.json";
import treasuryIdl from "@/idl/npcs_treasury.json";
import railsIdl from "@/idl/payment_rails.json";
import { ASSOCIATED_TOKEN_PROGRAM, ata, pdas, PROGRAMS, SYSTEM_PROGRAM, TOKEN_2022_PROGRAM, TOKEN_PROGRAM } from "@/lib/programs";
import {
  addCollateralIx,
  createAtaIx,
  owedAt,
  pledgeAndAdvanceIx,
  releaseIx,
  repayIx,
  requestRedemptionIx,
  trancheDepositIx,
  treasuryDepositIx,
  treasuryRedeemIx,
  treasuryScale,
  type FacilityRef,
} from "@/lib/flows";

interface IdlAcc {
  name: string;
  writable?: boolean;
  signer?: boolean;
  address?: string;
  pda?: { seeds: { kind: string; value?: number[]; path?: string }[] };
  accounts?: IdlAcc[];
}
type IdlLike = { address: string; instructions: { name: string; discriminator: number[]; accounts: IdlAcc[]; args: { name: string; type: unknown }[] }[] };
const flat = (as: IdlAcc[]): IdlAcc[] => as.flatMap((a) => (a.accounts ? flat(a.accounts) : [a]));
const idlIx = (idl: IdlLike, name: string) => idl.instructions.find((i) => i.name === name)!;

const writable = (r: AccountRole) => r === AccountRole.WRITABLE || r === AccountRole.WRITABLE_SIGNER;
const signer = (r: AccountRole) => r === AccountRole.READONLY_SIGNER || r === AccountRole.WRITABLE_SIGNER;

/**
 * `ix.accounts[offset..]` against instruction `name` of `idl`: same count
 * (unless more follow), same addresses (`expected`, else the IDL's fixed
 * `address`), same writability, same signer flag except for `deSigned`.
 */
function pin(ix: Instruction, idl: IdlLike, name: string, expected: Record<string, Address>, offset = 0, deSigned: string[] = []) {
  const metas = flat(idlIx(idl, name).accounts);
  const accs = ix.accounts!.slice(offset, offset + metas.length);
  expect(accs, name).toHaveLength(metas.length);
  metas.forEach((m, i) => {
    const want = expected[m.name] ?? m.address;
    expect(want, `${name}.${m.name}: no expectation`).toBeDefined();
    expect(accs[i]!.address, `${name}.${m.name}`).toBe(want);
    expect(writable(accs[i]!.role), `${name}.${m.name} writable`).toBe(!!m.writable);
    expect(signer(accs[i]!.role), `${name}.${m.name} signer`).toBe(!!m.signer && !deSigned.includes(m.name));
  });
  return metas.length;
}

/** Data = discriminator ‖ args (u64 / i64 little-endian, in IDL order). */
function data(idl: IdlLike, name: string, args: Record<string, bigint>): number[] {
  const i = idlIx(idl, name);
  const out = [...i.discriminator];
  for (const a of i.args) {
    const b = new DataView(new ArrayBuffer(8));
    if (a.type === "u64") b.setBigUint64(0, args[a.name]!, true);
    else if (a.type === "i64") b.setBigInt64(0, args[a.name]!, true);
    else throw new Error(`test encoder: ${String(a.type)}`);
    out.push(...new Uint8Array(b.buffer));
  }
  return out;
}

/** A PDA from the IDL's own `const` seeds (the program's seed bytes, not ours). */
async function idlPda(idl: IdlLike, ixName: string, acc: string, dynamic: Uint8Array[]): Promise<Address> {
  const m = flat(idlIx(idl, ixName).accounts).find((a) => a.name === acc)!;
  let d = 0;
  const seeds = m.pda!.seeds.map((s) => (s.kind === "const" ? Uint8Array.from(s.value!) : dynamic[d++]!));
  const [a] = await getProgramDerivedAddress({ programAddress: address(idl.address), seeds });
  return a;
}

const enc = getAddressEncoder();
const A = (a: Address) => enc.encode(a) as Uint8Array;
const le = (n: bigint) => {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigUint64(0, n, true);
  return b;
};
const KEYS = [
  "11111111111111111111111111111112",
  "SysvarC1ock11111111111111111111111111111111",
  "SysvarRent111111111111111111111111111111111",
  "Stake11111111111111111111111111111111111111",
  "Vote111111111111111111111111111111111111111",
  "Config1111111111111111111111111111111111111",
  "BPFLoader1111111111111111111111111111111111",
  "BPFLoader2111111111111111111111111111111111",
  "NativeLoader1111111111111111111111111111111",
  "Sysvar1111111111111111111111111111111111111",
  "SysvarRecentB1ockHashes11111111111111111111",
  "SysvarS1otHashes111111111111111111111111111",
  "SysvarStakeHistory1111111111111111111111111",
];
const k = (n: number) => address(KEYS[n]!);

describe("queue-claim request_redemption", () => {
  it("matches the IDL: accounts, PDAs (queue, request #id, vault, signer), qty and window date", async () => {
    const mint = k(0);
    const holder = k(1);
    const ix = await requestRedemptionIx(
      { assetMint: mint, nextId: 7n, holder, holderAsset: k(2), assetTokenProgram: TOKEN_2022_PROGRAM },
      1_500_000n,
      1_793_491_200n,
    );
    expect(ix.programAddress).toBe(PROGRAMS.queue);
    const queue = await idlPda(queueIdl as IdlLike, "request_redemption", "queue", [A(mint)]);
    const request = await idlPda(queueIdl as IdlLike, "request_redemption", "request", [A(queue), le(7n)]);
    expect(queue).toBe(await pdas.queue(mint));
    pin(ix, queueIdl as IdlLike, "request_redemption", {
      queue,
      request,
      npcs_vault: await idlPda(queueIdl as IdlLike, "request_redemption", "npcs_vault", [A(request)]),
      signer: await idlPda(queueIdl as IdlLike, "request_redemption", "signer", []),
      holder,
      holder_asset: k(2),
      asset_mint: mint,
      asset_token_program: TOKEN_2022_PROGRAM,
      queue_config: await idlPda(queueIdl as IdlLike, "request_redemption", "queue_config", [A(queue)]),
      // QC-P1 (#202): no attestation = anchor's `None` (the program id).
      attestation: PROGRAMS.queue,
    });
    expect([...ix.data!]).toEqual(data(queueIdl as IdlLike, "request_redemption", { qty: 1_500_000n, window_date: 1_793_491_200n }));
  });

  it("passes the holder's SAS redemption_request attestation when the queue requires one (QC-P1)", async () => {
    const ix = await requestRedemptionIx(
      { assetMint: k(0), nextId: 0n, holder: k(1), holderAsset: k(2), assetTokenProgram: TOKEN_2022_PROGRAM, attestation: k(6) },
      1n,
      1n,
    );
    const last = ix.accounts![ix.accounts!.length - 1]!;
    expect(last.address).toBe(k(6));
    expect(last.role).toBe(AccountRole.READONLY);
  });

  it("encodes a pre-epoch window date as a signed i64", async () => {
    const ix = await requestRedemptionIx(
      { assetMint: k(0), nextId: 0n, holder: k(1), holderAsset: k(2), assetTokenProgram: TOKEN_PROGRAM },
      1n,
      -1n,
    );
    expect([...ix.data!.slice(16)]).toEqual([255, 255, 255, 255, 255, 255, 255, 255]);
  });
});

describe("liquidity-facility", () => {
  const f: FacilityRef = {
    config: k(3),
    facilityId: 2n,
    assetMint: k(0),
    usdcMint: k(4),
    feed: k(5),
    assetTokenProgram: TOKEN_2022_PROGRAM,
    usdcTokenProgram: TOKEN_PROGRAM,
  };
  const borrower = k(6);

  async function facilityKeys() {
    const lfSigner = await idlPda(facilityIdl as IdlLike, "pledge_and_advance", "signer", []);
    const position = await idlPda(facilityIdl as IdlLike, "pledge_and_advance", "position", [A(f.config), A(borrower)]);
    const railsFacility = await pdas.railsFacility(lfSigner, f.facilityId);
    const holder = await pdas.pledgeHolder(f.assetMint, lfSigner, position);
    const gate = (blockOf: Address) =>
      Promise.all([pdas.gateConfig(f.assetMint), pdas.custodian(f.assetMint, lfSigner), pdas.lots(f.assetMint, holder), pdas.lots(f.assetMint, borrower), pdas.blocked(f.assetMint, blockOf)]).then(
        ([gate_config, custodian_record, pledge_lots, other_lots, block_entry]) => ({ gate_config, custodian_record, pledge_lots, other_lots, block_entry }),
      );
    return { lfSigner, position, railsFacility, holder, gate };
  }

  it("pledge_and_advance: facility accounts, gate custody, then rails advance_funds (curator de-signed)", async () => {
    const { lfSigner, position, railsFacility, holder, gate } = await facilityKeys();
    expect(lfSigner).toBe(await pdas.lfSigner());
    const ix = await pledgeAndAdvanceIx({ ...f, borrower, borrowerAsset: k(7), destinationToken: k(8) }, 3_000_000n, 1_000_000n);
    const n = pin(ix, facilityIdl as IdlLike, "pledge_and_advance", {
      config: f.config,
      position,
      collateral_vault: await idlPda(facilityIdl as IdlLike, "pledge_and_advance", "collateral_vault", [A(position)]),
      proceeds: await idlPda(facilityIdl as IdlLike, "pledge_and_advance", "proceeds", [A(position)]),
      feed: f.feed,
      signer: lfSigner,
      borrower,
      borrower_asset: k(7),
      asset_mint: f.assetMint,
      usdc_mint: f.usdcMint,
      asset_token_program: TOKEN_2022_PROGRAM,
      token_program: TOKEN_PROGRAM,
      rails_program: PROGRAMS.rails,
      // The first pledge opens the pledge holder's ledger: its block entry is screened.
      ...(await gate(holder)),
    });
    const r = pin(
      ix,
      railsIdl as IdlLike,
      "advance_funds",
      {
        config: await pdas.railsConfig(),
        facility: railsFacility,
        advance: await idlPda(railsIdl as IdlLike, "advance_funds", "advance", [A(railsFacility), A(borrower)]),
        vault: await pdas.facilityVault(railsFacility),
        borrower,
        destination: await pdas.payout(borrower),
        destination_token: k(8),
        usdc_mint: f.usdcMint,
        curator: lfSigner,
        token_program: TOKEN_PROGRAM,
      },
      n,
      ["curator"],
    );
    // The engine checks exactly 11 forwarded accounts: [1] fund, [4] borrower, [8] its signer.
    expect(r).toBe(11);
    expect(ix.accounts).toHaveLength(n + 11);
    expect(ix.accounts![n + 4]!.address).toBe(borrower);
    expect(ix.accounts![n + 8]!.address).toBe(lfSigner);
    expect([...ix.data!]).toEqual(data(facilityIdl as IdlLike, "pledge_and_advance", { qty: 3_000_000n, amount: 1_000_000n }));
  });

  it("add_collateral and release: the borrower's ledger and block entry", async () => {
    const { lfSigner, position, railsFacility, gate } = await facilityKeys();
    const collateral = await pdas.collateral(position);
    const add = await addCollateralIx({ ...f, borrower, borrowerAsset: k(7) }, 500_000n);
    pin(add, facilityIdl as IdlLike, "add_collateral", {
      config: f.config,
      position,
      signer: lfSigner,
      collateral_vault: collateral,
      borrower,
      borrower_asset: k(7),
      asset_mint: f.assetMint,
      asset_token_program: TOKEN_2022_PROGRAM,
      ...(await gate(borrower)),
    });
    expect([...add.data!]).toEqual(data(facilityIdl as IdlLike, "add_collateral", { qty: 500_000n }));

    const rel = await releaseIx({ ...f, borrower, borrowerAsset: k(7) });
    pin(rel, facilityIdl as IdlLike, "release", {
      config: f.config,
      position,
      advance: await pdas.advance(railsFacility, borrower),
      signer: lfSigner,
      collateral_vault: collateral,
      borrower_asset: k(7),
      asset_mint: f.assetMint,
      asset_token_program: TOKEN_2022_PROGRAM,
      ...(await gate(borrower)),
    });
    expect([...rel.data!]).toEqual(data(facilityIdl as IdlLike, "release", {}));
  });

  it("repay is the rails' own instruction, paid by the borrower to the facility fund", async () => {
    const { lfSigner, railsFacility } = await facilityKeys();
    const ix = await repayIx({ ...f, borrower, payerUsdc: k(9) }, 250_000n);
    expect(ix.programAddress).toBe(PROGRAMS.rails);
    expect(railsFacility).toBe(
      await idlPda(railsIdl as IdlLike, "repay", "facility", [A(lfSigner), le(f.facilityId)]),
    );
    pin(ix, railsIdl as IdlLike, "repay", {
      facility: railsFacility,
      advance: await pdas.advance(railsFacility, borrower),
      vault: await pdas.facilityVault(railsFacility),
      curator: lfSigner,
      payer_token: k(9),
      payer: borrower,
      usdc_mint: f.usdcMint,
      token_program: TOKEN_PROGRAM,
    });
    expect([...ix.data!]).toEqual(data(railsIdl as IdlLike, "repay", { amount: 250_000n }));
  });
});

describe("tranche-engine deposit", () => {
  it("matches the IDL: pool, both tranche mints and the depositor's three accounts", async () => {
    const a = {
      pool: k(0),
      feed: k(1),
      seniorMint: k(2),
      juniorMint: k(3),
      collateralVault: k(4),
      collateralMint: k(5),
      depositor: k(6),
      depositorCollateral: k(7),
      depositorSenior: k(8),
      depositorJunior: k(9),
      trancheTokenProgram: TOKEN_PROGRAM,
      collateralTokenProgram: TOKEN_2022_PROGRAM,
    };
    const ix = await trancheDepositIx(a, 42n);
    pin(ix, trancheIdl as IdlLike, "deposit", {
      pool: a.pool,
      feed: a.feed,
      senior_mint: a.seniorMint,
      junior_mint: a.juniorMint,
      collateral_vault: a.collateralVault,
      collateral_mint: a.collateralMint,
      depositor: a.depositor,
      depositor_collateral: a.depositorCollateral,
      depositor_senior: a.depositorSenior,
      depositor_junior: a.depositorJunior,
      signer: await idlPda(trancheIdl as IdlLike, "deposit", "signer", []),
      tranche_token_program: TOKEN_PROGRAM,
      collateral_token_program: TOKEN_2022_PROGRAM,
    });
    expect([...ix.data!]).toEqual(data(trancheIdl as IdlLike, "deposit", { qty: 42n }));
  });
});

describe("npcs-treasury", () => {
  const t = {
    assetMint: k(0),
    usdcMint: k(1),
    vault: k(2),
    owner: k(3),
    ownerUsdc: k(4),
    ownerAsset: k(5),
    usdcTokenProgram: TOKEN_PROGRAM,
    assetTokenProgram: TOKEN_2022_PROGRAM,
  };

  it("deposit: USDC in from the depositor, NPCS minted to the depositor", async () => {
    const treasury = await idlPda(treasuryIdl as IdlLike, "deposit", "treasury", [A(t.assetMint)]);
    const ix = await treasuryDepositIx(t, 1_000_000n);
    pin(ix, treasuryIdl as IdlLike, "deposit", {
      treasury,
      vault: t.vault,
      signer: await idlPda(treasuryIdl as IdlLike, "deposit", "signer", [A(treasury)]),
      asset_mint: t.assetMint,
      usdc_mint: t.usdcMint,
      depositor: t.owner,
      depositor_usdc: t.ownerUsdc,
      depositor_asset: t.ownerAsset,
      token_program: TOKEN_PROGRAM,
      asset_token_program: TOKEN_2022_PROGRAM,
    });
    expect([...ix.data!]).toEqual(data(treasuryIdl as IdlLike, "deposit", { amount: 1_000_000n }));
  });

  it("redeem: the authority burns its NPCS at the feed's NAV (asset account before USDC, as the IDL orders them)", async () => {
    const treasury = await pdas.treasury(t.assetMint);
    const ix = await treasuryRedeemIx(t, 1_000_000n);
    pin(ix, treasuryIdl as IdlLike, "redeem", {
      treasury,
      vault: t.vault,
      signer: await pdas.treasurySigner(treasury),
      asset_mint: t.assetMint,
      usdc_mint: t.usdcMint,
      authority: t.owner,
      authority_asset: t.ownerAsset,
      authority_usdc: t.ownerUsdc,
      token_program: TOKEN_PROGRAM,
      asset_token_program: TOKEN_2022_PROGRAM,
      // doc 09 Path 8 (#202): the redemption NAV's feed.
      feed: await pdas.feed(t.assetMint),
    });
    expect([...ix.data!]).toEqual(data(treasuryIdl as IdlLike, "redeem", { amount: 1_000_000n }));
    expect(treasuryScale(6, 6)).toBe(1n);
    expect(treasuryScale(9, 6)).toBe(1_000n);
  });
});

describe("associated token account", () => {
  it("CreateIdempotent opens the canonical ATA", async () => {
    const [want] = await getProgramDerivedAddress({
      programAddress: ASSOCIATED_TOKEN_PROGRAM,
      seeds: [A(k(1)), A(TOKEN_2022_PROGRAM), A(k(2))],
    });
    expect(await ata(k(1), k(2), TOKEN_2022_PROGRAM)).toBe(want);
    const ix = await createAtaIx(k(0), k(1), k(2), TOKEN_2022_PROGRAM);
    expect(ix.accounts!.map((m) => m.address)).toEqual([k(0), want, k(1), k(2), SYSTEM_PROGRAM, TOKEN_2022_PROGRAM]);
    expect(ix.accounts!.map((m) => m.role)).toEqual([
      AccountRole.WRITABLE_SIGNER,
      AccountRole.WRITABLE,
      AccountRole.READONLY,
      AccountRole.READONLY,
      AccountRole.READONLY,
      AccountRole.READONLY,
    ]);
    expect([...ix.data!]).toEqual([1]);
  });
});

describe("repay all", () => {
  it("owes principal + interest + accrual to a minute past now, rounded up", () => {
    // 1,000 USDC at 12% for one year less a minute → +120 USDC.
    expect(owedAt(1_000_000_000n, 5n, 0n, 1_200n, 31_536_000n - 60n)).toBe(1_000_000_000n + 5n + 120_000_000n);
    expect(owedAt(1n, 0n, 100n, 1_200n, 100n)).toBe(2n); // any accrual rounds up
    expect(owedAt(0n, 7n, 100n, 1_200n, 0n)).toBe(7n);
  });
});
