/**
 * Off-chain mirror of the on-chain refund rules (`CashSession::refund_route`
 * / `bond_forfeit` in `state.rs`). The crank uses it to decide what is
 * refundable *before* spending a transaction; the program remains the
 * authority. `tests/rules.spec.ts` pins it to the same truth table as the
 * Rust unit tests, so the two can't drift silently.
 */

export type SessionStatus = "Open" | "Failed" | "Settled";

export interface SessionTimes {
  status: SessionStatus;
  revealEnd: bigint;
  settleDeadline: bigint;
  failedAt: bigint;
}

export type BondRoute = "ToOwner" | "ToReserve";

export function bondForfeit(s: SessionTimes, revealed: boolean, now: bigint): boolean {
  if (revealed || now < s.revealEnd) return false;
  return s.status === "Failed" ? s.failedAt >= s.revealEnd : true;
}

/** `null` = not refundable yet (the program would say `DeadlineNotReached`). */
export function refundRoute(s: SessionTimes, revealed: boolean, now: bigint): BondRoute | null {
  const refundable =
    s.status !== "Open" || now >= s.settleDeadline || (!revealed && now >= s.revealEnd);
  if (!refundable) return null;
  return bondForfeit(s, revealed, now) ? "ToReserve" : "ToOwner";
}
