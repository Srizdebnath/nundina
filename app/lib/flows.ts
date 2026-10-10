/**
 * Participant writes outside the auction (#202): a redemption-queue
 * request (queue-claim), a facility pledge / advance / repayment / top-up /
 * release (liquidity-facility + payment-rails), a tranche deposit
 * (tranche-engine) and an NPCS deposit / redeem (npcs-treasury).
 *
 * Built in the browser and signed by the participant's own wallet, like
 * `tx.ts`; the server never signs. Account order and writability are the
 * programs' IDLs (pinned by `test/flows.test.ts`); data is the IDL
 * discriminator followed by the Borsh arguments.
 */
import { AccountRole, type Address, type Instruction } from "@solana/kit";
import { concat, u64le, type Idl } from "./borsh";
import { ASSOCIATED_TOKEN_PROGRAM, ata, codecs, pdas, PROGRAMS, SYSTEM_PROGRAM } from "./programs";

const disc = (idl: Idl, name: string) => {
  const i = idl.instructions.find((x) => x.name === name);
  if (!i) throw new Error(`no instruction ${name} in ${idl.metadata?.name ?? idl.address}`);
  return Uint8Array.from(i.discriminator);
};
const ro = (address: Address) => ({ address, role: AccountRole.READONLY });
const w = (address: Address) => ({ address, role: AccountRole.WRITABLE });
const ws = (address: Address) => ({ address, role: AccountRole.WRITABLE_SIGNER });
const rs = (address: Address) => ({ address, role: AccountRole.READONLY_SIGNER });

export function i64le(n: bigint): Uint8Array {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigInt64(0, n, true);
  return b;
}

/** Associated Token `CreateIdempotent`: opens `owner`'s account for `mint` if it is missing. */
export async function createAtaIx(payer: Address, owner: Address, mint: Address, tokenProgram: Address): Promise<Instruction> {
  return {
    programAddress: ASSOCIATED_TOKEN_PROGRAM,
    accounts: [ws(payer), w(await ata(owner, mint, tokenProgram)), ro(owner), ro(mint), ro(SYSTEM_PROGRAM), ro(tokenProgram)],
    data: Uint8Array.of(1),
  };
}

// ── queue-claim (#138) ──────────────────────────────────────────────────

export interface RedemptionAccounts {
  /** The queue's asset (NPCS); the queue is `["queue", mint]`. */
  assetMint: Address;
  /** `queue.requests`: the next request id, part of the request PDA. */
  nextId: bigint;
  holder: Address;
  holderAsset: Address;
  assetTokenProgram: Address;
}

/**
 * QC-P1 `request_redemption(qty, window_date)`: `qty` NPCS move into the
 * request's own vault and the request waits for the attester (`attest`
 * mints the frozen claim token and sets the rank and fill schedule).
 */
export async function requestRedemptionIx(a: RedemptionAccounts, qty: bigint, windowDate: bigint): Promise<Instruction> {
  const queue = await pdas.queue(a.assetMint);
  const request = await pdas.queueRequest(queue, a.nextId);
  return {
    programAddress: PROGRAMS.queue,
    accounts: [
      w(queue),
      w(request),
      w(await pdas.queueNpcsVault(request)),
      ro(await pdas.queueSigner()),
      ws(a.holder),
      w(a.holderAsset),
      ro(a.assetMint),
      ro(a.assetTokenProgram),
      ro(SYSTEM_PROGRAM),
    ],
    data: concat(disc(codecs.queue.idl, "request_redemption"), u64le(qty), i64le(windowDate)),
  };
}

// ── liquidity-facility (#139) ───────────────────────────────────────────

/** What every facility write needs from the facility's `FacilityConfig`. */
export interface FacilityRef {
  config: Address;
  facilityId: bigint;
  assetMint: Address;
  usdcMint: Address;
  feed: Address;
  assetTokenProgram: Address;
  usdcTokenProgram: Address;
}

/** The rails accounts of a facility: its fund, vault, and this borrower's advance. */
export async function railsOf(f: FacilityRef, borrower: Address) {
  const signer = await pdas.lfSigner();
  const facility = await pdas.railsFacility(signer, f.facilityId);
  return {
    signer,
    facility,
    vault: await pdas.facilityVault(facility),
    advance: await pdas.advance(facility, borrower),
  };
}

/**
 * LF-P5 `GateCustody`: the position's pledge ledger, plus `other`'s ledger
 * and the block entry of `blockOf` (the borrower's, except on the first
 * pledge, where the gate opens the pledge holder's ledger and screens it).
 */
async function gateCustody(f: FacilityRef, position: Address, other: Address, blockOf: Address) {
  const signer = await pdas.lfSigner();
  const holder = await pdas.pledgeHolder(f.assetMint, signer, position);
  return [
    ro(PROGRAMS.gate),
    ro(await pdas.gateConfig(f.assetMint)),
    ro(await pdas.custodian(f.assetMint, signer)),
    w(await pdas.lots(f.assetMint, holder)),
    w(await pdas.lots(f.assetMint, other)),
    ro(await pdas.blocked(f.assetMint, blockOf === position ? holder : blockOf)),
  ];
}

export interface PledgeAccounts extends FacilityRef {
  borrower: Address;
  borrowerAsset: Address;
  /** The borrower's registered payout destination token account (rails `PayoutDestination.token_account`). */
  destinationToken: Address;
}

/**
 * LF-P1 `pledge_and_advance(qty, amount)`: pledge `qty` of the collateral
 * (a gated asset's lots move to the position's pledge ledger) and draw
 * `amount` USDC, valued at mark-engine `MARK_PROTECTIVE` × the curator rate.
 * The 11 forwarded accounts are rails `advance_funds`, the facility signer
 * PDA de-signed (the engine signs for it).
 */
export async function pledgeAndAdvanceIx(a: PledgeAccounts, qty: bigint, amount: bigint): Promise<Instruction> {
  const position = await pdas.position(a.config, a.borrower);
  const r = await railsOf(a, a.borrower);
  return {
    programAddress: PROGRAMS.facility,
    accounts: [
      ro(a.config),
      w(position),
      w(await pdas.collateral(position)),
      w(await pdas.proceeds(position)),
      ro(a.feed),
      w(r.signer),
      ws(a.borrower),
      w(a.borrowerAsset),
      ro(a.assetMint),
      ro(a.usdcMint),
      ro(a.assetTokenProgram),
      ro(a.usdcTokenProgram),
      ro(PROGRAMS.rails),
      ro(SYSTEM_PROGRAM),
      ...(await gateCustody(a, position, a.borrower, position)),
      // payment-rails `advance_funds`, forwarded.
      ro(await pdas.railsConfig()),
      w(r.facility),
      w(r.advance),
      w(r.vault),
      ro(a.borrower),
      ro(await pdas.payout(a.borrower)),
      w(a.destinationToken),
      ro(a.usdcMint),
      w(r.signer),
      ro(a.usdcTokenProgram),
      ro(SYSTEM_PROGRAM),
    ],
    data: concat(disc(codecs.facility.idl, "pledge_and_advance"), u64le(qty), u64le(amount)),
  };
}

/** `add_collateral(qty)`: top up an Active or Breached position (cures at the next `check_health`). */
export async function addCollateralIx(a: FacilityRef & { borrower: Address; borrowerAsset: Address }, qty: bigint): Promise<Instruction> {
  const position = await pdas.position(a.config, a.borrower);
  return {
    programAddress: PROGRAMS.facility,
    accounts: [
      ro(a.config),
      w(position),
      ro(await pdas.lfSigner()),
      w(await pdas.collateral(position)),
      rs(a.borrower),
      w(a.borrowerAsset),
      ro(a.assetMint),
      ro(a.assetTokenProgram),
      ...(await gateCustody(a, position, a.borrower, a.borrower)),
    ],
    data: concat(disc(codecs.facility.idl, "add_collateral"), u64le(qty)),
  };
}

/**
 * LF-P6: payment-rails `repay(amount)`, paid by the borrower straight to the
 * facility fund (interest first, then principal). The facility signer is
 * the fund's curator, named but not signing.
 */
export async function repayIx(a: FacilityRef & { borrower: Address; payerUsdc: Address }, amount: bigint): Promise<Instruction> {
  const r = await railsOf(a, a.borrower);
  return {
    programAddress: PROGRAMS.rails,
    accounts: [w(r.facility), w(r.advance), w(r.vault), w(r.signer), w(a.payerUsdc), rs(a.borrower), ro(a.usdcMint), ro(a.usdcTokenProgram)],
    data: concat(disc(codecs.rails.idl, "repay"), u64le(amount)),
  };
}

/** `release`: once the rails advance is closed, the collateral (and its lots) go back. Permissionless. */
export async function releaseIx(a: FacilityRef & { borrower: Address; borrowerAsset: Address }): Promise<Instruction> {
  const position = await pdas.position(a.config, a.borrower);
  const r = await railsOf(a, a.borrower);
  return {
    programAddress: PROGRAMS.facility,
    accounts: [
      ro(a.config),
      w(position),
      ro(r.advance),
      ro(r.signer),
      w(await pdas.collateral(position)),
      w(a.borrowerAsset),
      ro(a.assetMint),
      ro(a.assetTokenProgram),
      ...(await gateCustody(a, position, a.borrower, a.borrower)),
    ],
    data: disc(codecs.facility.idl, "release"),
  };
}

// ── tranche-engine (#140) ───────────────────────────────────────────────

export interface TrancheDepositAccounts {
  pool: Address;
  feed: Address;
  seniorMint: Address;
  juniorMint: Address;
  collateralVault: Address;
  collateralMint: Address;
  depositor: Address;
  depositorCollateral: Address;
  depositorSenior: Address;
  depositorJunior: Address;
  trancheTokenProgram: Address;
  collateralTokenProgram: Address;
}

/**
 * TE-P2 `deposit(qty)`: `qty` collateral into the pool at `MARK_PROTECTIVE`
 * (Fresh mark only). One deposit mints both tranches: junior = value ×
 * attachment (frozen for the lock-up), senior = the rest.
 */
export async function trancheDepositIx(a: TrancheDepositAccounts, qty: bigint): Promise<Instruction> {
  return {
    programAddress: PROGRAMS.tranche,
    accounts: [
      w(a.pool),
      ro(a.feed),
      w(a.seniorMint),
      w(a.juniorMint),
      w(a.collateralVault),
      ro(a.collateralMint),
      rs(a.depositor),
      w(a.depositorCollateral),
      w(a.depositorSenior),
      w(a.depositorJunior),
      ro(await pdas.teSigner()),
      ro(a.trancheTokenProgram),
      ro(a.collateralTokenProgram),
    ],
    data: concat(disc(codecs.tranche.idl, "deposit"), u64le(qty)),
  };
}

// ── npcs-treasury (#148) ────────────────────────────────────────────────

export interface TreasuryAccounts {
  assetMint: Address;
  usdcMint: Address;
  vault: Address;
  /** The depositor (deposit) or the treasury authority (redeem). */
  owner: Address;
  ownerUsdc: Address;
  ownerAsset: Address;
  usdcTokenProgram: Address;
  assetTokenProgram: Address;
}

/**
 * I1 `deposit(amount)`: `amount` USDC into the treasury vault, the same
 * value of NPCS minted to the depositor (whose NPCS account must already be
 * gate-thawed: NPCS is default-frozen).
 */
export async function treasuryDepositIx(a: TreasuryAccounts, amount: bigint): Promise<Instruction> {
  const treasury = await pdas.treasury(a.assetMint);
  return {
    programAddress: PROGRAMS.treasury,
    accounts: [
      w(treasury),
      w(a.vault),
      ro(await pdas.treasurySigner(treasury)),
      w(a.assetMint),
      ro(a.usdcMint),
      rs(a.owner),
      w(a.ownerUsdc),
      w(a.ownerAsset),
      ro(a.usdcTokenProgram),
      ro(a.assetTokenProgram),
    ],
    data: concat(disc(codecs.treasury.idl, "deposit"), u64le(amount)),
  };
}

/** `redeem(amount)`: the treasury **authority** burns `amount` NPCS it holds and takes the USDC out. */
export async function treasuryRedeemIx(a: TreasuryAccounts, amount: bigint): Promise<Instruction> {
  const treasury = await pdas.treasury(a.assetMint);
  return {
    programAddress: PROGRAMS.treasury,
    accounts: [
      w(treasury),
      w(a.vault),
      ro(await pdas.treasurySigner(treasury)),
      w(a.assetMint),
      ro(a.usdcMint),
      rs(a.owner),
      w(a.ownerAsset),
      w(a.ownerUsdc),
      ro(a.usdcTokenProgram),
      ro(a.assetTokenProgram),
    ],
    data: concat(disc(codecs.treasury.idl, "redeem"), u64le(amount)),
  };
}

/** NPCS base units per USDC base unit (treasury `scale`: 10^(asset dp − usdc dp)). */
export const treasuryScale = (assetDecimals: number, usdcDecimals: number) => 10n ** BigInt(assetDecimals - usdcDecimals);

const SECONDS_PER_YEAR = 31_536_000n;

/**
 * What a rails `Advance` will owe at `now` (credit-math `accrue_interest`:
 * simple, per second, floored), rounded up by a minute of interest so a
 * "repay all" sent now still clears it. The rails take at most what is owed.
 */
export function owedAt(principal: bigint, interest: bigint, lastAccrual: bigint, rateBps: bigint, now: bigint): bigint {
  const dt = now + 60n > lastAccrual ? now + 60n - lastAccrual : 0n;
  const more = (principal * rateBps * dt + 10_000n * SECONDS_PER_YEAR - 1n) / (10_000n * SECONDS_PER_YEAR);
  return principal + interest + more;
}
