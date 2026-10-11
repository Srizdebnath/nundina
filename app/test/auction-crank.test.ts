/**
 * The auction crank: clear, expire, allocate, screen_bid, screen_ask, finish,
 * commit_ask and settle.
 *
 * These are the instructions that let a session actually reach a clearing
 * price from the UI. Before they existed the dApp could commit and reveal but
 * nothing else — the book sat at `Reveal` with no price, no fills and no print.
 *
 * Every builder is pinned to the program's own IDL, and the CPI-forwarded
 * account blocks are pinned to the rails and gate IDLs at their offsets, so a
 * reordering in any one of the three programs fails here rather than on chain.
 *
 * Every account the builder derives from a seed is asserted against the same
 * `pdas` helper the builder uses, so a seed change is a test failure, not a
 * runtime one.
 */
import { describe, expect, it } from "vitest";
import { AccountRole, type Address } from "@solana/kit";
import auctionIdl from "@/idl/auction_engine.json";
import railsIdl from "@/idl/payment_rails.json";
import gateIdl from "@/idl/credit_gate.json";
import {
  allocateIx,
  clearIx,
  commitAskIx,
  expireIx,
  finishIx,
  screenAskIx,
  screenBidIx,
  settleIx,
} from "@/lib/tx";
import { PROGRAMS, SYSTEM_PROGRAM, TOKEN_2022_PROGRAM, TOKEN_PROGRAM, pdas } from "@/lib/programs";
import { data, flat, idlIx, k, pin } from "./pin";

const auc = auctionIdl as never;
const rails = railsIdl as never;
const gate = gateIdl as never;

const assetMint = k(0);
const usdcMint = k(1);
const session = k(2);
const railsSession = k(3);
const buyer = k(4);
const seller = k(5);
const payer = k(6);
const buyerAsset = k(7);
const sellerUsdc = k(8);
const ownerAsset = k(9);
const ownerUsdc = k(10);
const feeReceiver = k(11);
const reserve = k(12);
const feed = k(13);
const printAuthority = k(14);
const identitySchema = k(15);
const accreditationSchema = k(16);
const credential = k(17);

const investorId = new Uint8Array(32).fill(7);
const hash = new Uint8Array(32).fill(9);
const isSigner = (r: AccountRole) => r === AccountRole.READONLY_SIGNER || r === AccountRole.WRITABLE_SIGNER;

describe("auction clear", () => {
  it("is permissionless: venue, session, feed, operator, rails — no signer at all", async () => {
    const ix = await clearIx(session, feed);
    pin(ix, auc, "clear", {
      venue: await pdas.venue(),
      session,
      feed,
      operator: await pdas.operator(),
      rails_program: PROGRAMS.rails,
    });
    expect(Array.from(ix.data as Uint8Array)).toEqual(data(auc, "clear", {}));
    expect(ix.accounts!.some((a) => isSigner(a.role))).toBe(false);
  });
});

describe("auction expire", () => {
  it("matches the IDL and takes no arguments", async () => {
    const ix = await expireIx(session, feed);
    pin(ix, auc, "expire", {
      venue: await pdas.venue(),
      session,
      feed,
      operator: await pdas.operator(),
      rails_program: PROGRAMS.rails,
    });
    expect(Array.from(ix.data as Uint8Array)).toEqual(data(auc, "expire", {}));
  });
});

describe("auction allocate", () => {
  it("takes only the session and encodes max_fills as u8", () => {
    const ix = allocateIx(session, 24);
    pin(ix, auc, "allocate", { session });
    expect(Array.from(ix.data as Uint8Array)).toEqual([...idlIx(auc, "allocate").discriminator, 24]);
  });
});

describe("auction screen_bid", () => {
  const args = {
    session,
    assetMint,
    buyer,
    orderIndex: 3,
    investorId,
    credential,
    identitySchema,
    accreditationSchema,
  };
  it("pins every gate account to its PDA and encodes order_index as u8", async () => {
    const ix = await screenBidIx(args);
    pin(ix, auc, "screen_bid", {
      session,
      gate_config: await pdas.gateConfig(assetMint),
      gate_policy: await pdas.gatePolicy(assetMint),
      block_entry: await pdas.blocked(assetMint, buyer),
      holder_link: await pdas.holderLink(assetMint, buyer),
      investor: await pdas.investor(assetMint, investorId),
      buyer_lots: await pdas.lots(assetMint, buyer),
      identity_attestation: await pdas.attestation(credential, identitySchema, buyer),
      accreditation_attestation: await pdas.attestation(credential, accreditationSchema, buyer),
      mint: assetMint,
      gate_program: PROGRAMS.gate,
    });
    expect(Array.from(ix.data as Uint8Array)).toEqual([...idlIx(auc, "screen_bid").discriminator, 3]);
  });

  it("takes no signer — AE-P6 makes an ineligible order an exclusion, not an error", async () => {
    const ix = await screenBidIx({ ...args, orderIndex: 0 });
    expect(ix.accounts!.some((a) => isSigner(a.role))).toBe(false);
  });
});

describe("auction screen_ask", () => {
  it("pins the seller-side gate block", async () => {
    const ix = await screenAskIx({ session, assetMint, seller, orderIndex: 0 });
    pin(ix, auc, "screen_ask", {
      session,
      gate_config: await pdas.gateConfig(assetMint),
      block_entry: await pdas.blocked(assetMint, seller),
      seller_lots: await pdas.lots(assetMint, seller),
      holder_link: await pdas.holderLink(assetMint, seller),
    });
    expect(Array.from(ix.data as Uint8Array)).toEqual([...idlIx(auc, "screen_ask").discriminator, 0]);
  });
});

describe("auction finish", () => {
  it("writes the print record and takes a payer for its rent", async () => {
    const ix = await finishIx({ session, feed, assetMint, printAuthority, payer });
    pin(ix, auc, "finish", {
      venue: await pdas.venue(),
      session,
      feed,
      print_record: await pdas.printRecord(feed, session),
      print_authority: printAuthority,
      operator: await pdas.operator(),
      payer,
      rails_program: PROGRAMS.rails,
      mark_program: PROGRAMS.mark,
      system_program: SYSTEM_PROGRAM,
    });
    expect(Array.from(ix.data as Uint8Array)).toEqual(data(auc, "finish", {}));
    expect(ix.accounts!.find((a) => a.role === AccountRole.WRITABLE_SIGNER)!.address).toBe(payer);
  });
});

describe("auction commit_ask", () => {
  it("forwards the rails deposit_ask block, and commits the qty", async () => {
    const ix = await commitAskIx(
      {
        session,
        railsSession,
        owner: seller,
        usdcMint,
        assetMint,
        ownerUsdc,
        ownerAsset,
        usdcTokenProgram: TOKEN_PROGRAM,
        assetTokenProgram: TOKEN_2022_PROGRAM,
      },
      hash,
      1_000_000n,
    );
    pin(ix, auc, "commit_ask", { session, owner: seller, rails_program: PROGRAMS.rails });

    // 3 auction accounts, then all 13 rails `deposit_ask` accounts in IDL order.
    const n = flat(idlIx(rails, "deposit_ask").accounts).length;
    expect(n).toBe(13);
    expect(ix.accounts).toHaveLength(3 + n);

    const escrow = await pdas.escrow(railsSession, seller);
    const forwarded: Record<string, Address> = {
      config: await pdas.railsConfig(),
      session: railsSession,
      escrow,
      vault: await pdas.escrowVault(escrow),
      asset_vault: await pdas.assetVault(escrow),
      usdc_mint: usdcMint,
      asset_mint: assetMint,
      owner_usdc: ownerUsdc,
      owner_asset: ownerAsset,
      owner: seller,
      token_program: TOKEN_PROGRAM,
      asset_token_program: TOKEN_2022_PROGRAM,
      system_program: SYSTEM_PROGRAM,
    };
    flat(idlIx(rails, "deposit_ask").accounts).forEach((m, i) => {
      expect(ix.accounts![3 + i]!.address, `deposit_ask.${m.name}`).toBe(forwarded[m.name]);
    });

    const qty = new Uint8Array(8);
    new DataView(qty.buffer).setBigUint64(0, 1_000_000n, true);
    expect(Array.from(ix.data as Uint8Array)).toEqual([...idlIx(auc, "commit_ask").discriminator, ...hash, ...qty]);
  });
});

describe("auction settle", () => {
  const accounts = {
    session,
    railsSession,
    assetMint,
    usdcMint,
    buyer,
    seller,
    buyerAsset,
    sellerUsdc,
    feeReceiver,
    reserve,
    buyerTokenProgram: TOKEN_PROGRAM,
    assetTokenProgram: TOKEN_2022_PROGRAM,
    payer,
    buyerInvestorId: investorId,
    credential,
    identitySchema,
    accreditationSchema,
  };

  it("forwards settle_fill then settle_check, each block in IDL order", async () => {
    const ix = await settleIx(accounts, 5);
    const operator = await pdas.operator();
    pin(
      ix,
      auc,
      "settle",
      {
        venue: await pdas.venue(),
        session,
        operator,
        payer,
        rails_program: PROGRAMS.rails,
        system_program: SYSTEM_PROGRAM,
      },
      0,
      ["operator"],
    );

    // ── rails `settle_fill`, offset 6 ────────────────────────────────────
    // The rails IDL declares 18 top-level accounts where #17 is `gate`, which
    // itself carries the 11 `settle_check` accounts nested. `flat()` drops a
    // parent that has children, so the block is pinned positionally:
    // 6 engine accounts, then 18 rails accounts (gate at 17), then 11 gate ones.
    const fillMetas = idlIx(rails, "settle_fill").accounts;
    expect(fillMetas).toHaveLength(18);
    const buyerEscrow = await pdas.escrow(railsSession, buyer);
    const sellerEscrow = await pdas.escrow(railsSession, seller);
    const fill: Record<string, Address> = {
      config: await pdas.railsConfig(),
      session: railsSession,
      operator,
      receipt: await pdas.receipt(railsSession, 5),
      buyer_escrow: buyerEscrow,
      buyer_vault: await pdas.escrowVault(buyerEscrow),
      seller_escrow: sellerEscrow,
      seller_asset_vault: await pdas.assetVault(sellerEscrow),
      seller_usdc: sellerUsdc,
      buyer_asset: buyerAsset,
      fee_receiver: feeReceiver,
      reserve,
      usdc_mint: usdcMint,
      asset_mint: assetMint,
      token_program: TOKEN_PROGRAM,
      asset_token_program: TOKEN_2022_PROGRAM,
      system_program: SYSTEM_PROGRAM,
      gate: PROGRAMS.gate,
    };
    fillMetas.forEach((m, i) => {
      expect(ix.accounts![6 + i]!.address, `settle_fill.${m.name}`).toBe(fill[m.name]);
    });

    // ── credit-gate `settle_check`, offset 24, 11 accounts ───────────────
    const checkMetas = flat(idlIx(gate, "settle_check").accounts);
    expect(checkMetas).toHaveLength(11);
    const check: Record<string, Address> = {
      settlement_authority: await pdas.gateSettle(),
      config: await pdas.gateConfig(assetMint),
      policy: await pdas.gatePolicy(assetMint),
      seller_block: await pdas.blocked(assetMint, seller),
      buyer_block: await pdas.blocked(assetMint, buyer),
      seller_lots: await pdas.lots(assetMint, seller),
      buyer_lots: await pdas.lots(assetMint, buyer),
      buyer_link: await pdas.holderLink(assetMint, buyer),
      identity_attestation: await pdas.attestation(credential, identitySchema, buyer),
      accreditation_attestation: await pdas.attestation(credential, accreditationSchema, buyer),
      mint: assetMint,
    };
    checkMetas.forEach((m, i) => {
      expect(ix.accounts![6 + 18 + i]!.address, `settle_check.${m.name}`).toBe(check[m.name]);
    });

    expect(ix.accounts).toHaveLength(6 + 18 + 11);
    expect(Array.from(ix.data as Uint8Array)).toEqual([...idlIx(auc, "settle").discriminator, 5, 0, 0, 0]);
  });

  it("de-signs the operator — the engine signs for it over CPI", async () => {
    const ix = await settleIx(accounts, 0);
    const operator = await pdas.operator();
    const ops = ix.accounts!.filter((a) => a.address === operator);
    expect(ops).toHaveLength(2);
    expect(ops.every((a) => a.role === AccountRole.WRITABLE)).toBe(true);
  });
});
