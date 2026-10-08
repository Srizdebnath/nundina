/**
 * The ROFR writes are pinned against the programs' own IDLs: every account
 * the page sends sits at the IDL's position with the IDL's writability, and
 * the forwarded rails list is `settle_rofr_fill`'s, operator de-signed.
 */
import { describe, expect, it } from "vitest";
import { AccountRole, address, type Address } from "@solana/kit";
import railsIdl from "@/idl/payment_rails.json";
import auctionIdl from "@/idl/auction_engine.json";
import { pdas, PROGRAMS, SYSTEM_PROGRAM, TOKEN_2022_PROGRAM, TOKEN_PROGRAM } from "@/lib/programs";
import { closeRofrIx, computeUnitLimitIx, exerciseRofrIx, type RofrAccounts } from "@/lib/tx";

interface IdlAcc {
  name: string;
  writable?: boolean;
  signer?: boolean;
  accounts?: IdlAcc[];
}
const flat = (as: IdlAcc[]): IdlAcc[] => as.flatMap((a) => (a.accounts ? flat(a.accounts) : [a]));
const ixAccounts = (idl: { instructions: { name: string; accounts: IdlAcc[] }[] }, name: string) =>
  flat(idl.instructions.find((i) => i.name === name)!.accounts);

const k = (n: number) => address(["11111111111111111111111111111112", "SysvarC1ock11111111111111111111111111111111", "SysvarRent111111111111111111111111111111111", "Stake11111111111111111111111111111111111111", "Vote111111111111111111111111111111111111111", "Config1111111111111111111111111111111111111", "BPFLoader1111111111111111111111111111111111", "BPFLoader2111111111111111111111111111111111", "NativeLoader1111111111111111111111111111111", "Sysvar1111111111111111111111111111111111111", "SysvarRecentB1ockHashes11111111111111111111", "SysvarS1otHashes111111111111111111111111111", "SysvarStakeHistory1111111111111111111111111"][n]!);

const a: RofrAccounts = {
  session: k(0),
  railsSession: k(1),
  issuer: k(2),
  seller: k(3),
  assetMint: k(4),
  usdcMint: k(5),
  feeReceiver: k(6),
  reserve: k(7),
  sellerUsdc: k(8),
  issuerUsdc: k(9),
  issuerAsset: k(10),
  usdcTokenProgram: TOKEN_PROGRAM,
  assetTokenProgram: TOKEN_2022_PROGRAM,
};

describe("exercise_rofr", () => {
  it("matches the auction-engine and payment-rails IDLs account for account", async () => {
    const ix = await exerciseRofrIx(a, 1, 3, 250_000n);
    const accs = ix.accounts!;
    const outer = ixAccounts(auctionIdl as never, "exercise_rofr");
    const inner = ixAccounts(railsIdl as never, "settle_rofr_fill");
    expect(inner).toHaveLength(23); // the engine requires exactly 23
    expect(accs).toHaveLength(outer.length + inner.length);

    const operator = await pdas.operator();
    const sellerEscrow = await pdas.escrow(a.railsSession, a.seller);
    const expected: Record<string, Address> = {
      // auction-engine ExerciseRofr
      venue: await pdas.venue(),
      session: a.session,
      issuer: a.issuer,
      operator,
      rails_program: PROGRAMS.rails,
      system_program: SYSTEM_PROGRAM,
    };
    outer.forEach((m, i) => {
      expect(accs[i]!.address, m.name).toBe(expected[m.name]);
      const writable = accs[i]!.role === AccountRole.WRITABLE || accs[i]!.role === AccountRole.WRITABLE_SIGNER;
      expect(writable, m.name).toBe(!!m.writable);
    });

    const railsExpected: Record<string, Address> = {
      config: await pdas.railsConfig(),
      session: a.railsSession,
      operator,
      issuer: a.issuer,
      receipt: await pdas.receipt(a.railsSession, 3), // the fill the engine appends
      seller_escrow: sellerEscrow,
      seller_asset_vault: await pdas.assetVault(sellerEscrow),
      seller_usdc: a.sellerUsdc,
      issuer_usdc: a.issuerUsdc,
      issuer_asset: a.issuerAsset,
      fee_receiver: a.feeReceiver,
      reserve: a.reserve,
      usdc_mint: a.usdcMint,
      asset_mint: a.assetMint,
      token_program: TOKEN_PROGRAM,
      asset_token_program: TOKEN_2022_PROGRAM,
      system_program: SYSTEM_PROGRAM,
      gate_program: PROGRAMS.gate,
      gate_authority: await pdas.gateSettle(),
      gate_config: await pdas.gateConfig(a.assetMint),
      seller_block: await pdas.blocked(a.assetMint, a.seller),
      issuer_block: await pdas.blocked(a.assetMint, a.issuer),
      seller_lots: await pdas.lots(a.assetMint, a.seller),
    };
    inner.forEach((m, j) => {
      const got = accs[outer.length + j]!;
      expect(got.address, m.name).toBe(railsExpected[m.name]);
      const writable = got.role === AccountRole.WRITABLE || got.role === AccountRole.WRITABLE_SIGNER;
      const signer = got.role === AccountRole.READONLY_SIGNER || got.role === AccountRole.WRITABLE_SIGNER;
      expect(writable, m.name).toBe(!!m.writable);
      // The engine signs for its operator PDA by CPI; only the issuer signs the transaction.
      expect(signer, m.name).toBe(m.name === "issuer");
    });

    const disc = auctionIdl.instructions.find((i) => i.name === "exercise_rofr")!.discriminator;
    expect([...ix.data!.slice(0, 8)]).toEqual(disc);
    expect([...ix.data!.slice(8)]).toEqual([1, 0, 0, 0, 0x90, 0xd0, 0x03, 0, 0, 0, 0, 0]);
  });

  it("close_rofr and the compute limit are the IDL / ComputeBudget layouts", () => {
    const ix = closeRofrIx(a.session, a.issuer);
    expect(ixAccounts(auctionIdl as never, "close_rofr").map((m) => m.name)).toEqual(["session", "caller"]);
    expect(ix.accounts!.map((m) => m.role)).toEqual([AccountRole.WRITABLE, AccountRole.READONLY_SIGNER]);
    expect([...computeUnitLimitIx(600_000).data!]).toEqual([2, 0xc0, 0x27, 0x09, 0]);
  });
});
