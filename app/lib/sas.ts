/**
 * Solana Attestation Service account bytes, decoded exactly as
 * `credit-gate` does (`programs/credit-gate/src/eligibility.rs`): one
 * discriminator byte, then `nonce | credential | schema | u32-len data |
 * signer | i64 expiry | token_account`.
 */
import { getAddressDecoder, type Address } from "@solana/kit";

export const SAS_ATTESTATION_DISCRIMINATOR = 2;

export interface Attestation {
  nonce: Address;
  credential: Address;
  schema: Address;
  data: Uint8Array;
  signer: Address;
  /** Unix seconds; 0 = never expires. */
  expiry: number;
}

const dec = getAddressDecoder();

export function parseAttestation(b: Uint8Array): Attestation | null {
  if (b.length < 1 + 96 + 4 || b[0] !== SAS_ATTESTATION_DISCRIMINATOR) return null;
  const view = new DataView(b.buffer, b.byteOffset, b.byteLength);
  let o = 1;
  const key = () => {
    const k = dec.decode(b.subarray(o, o + 32));
    o += 32;
    return k;
  };
  const nonce = key();
  const credential = key();
  const schema = key();
  const len = view.getUint32(o, true);
  o += 4;
  if (o + len + 32 + 8 + 32 !== b.length) return null;
  const data = b.slice(o, o + len);
  o += len;
  const signer = key();
  const expiry = Number(view.getBigInt64(o, true));
  return { nonce, credential, schema, data, signer, expiry };
}

/** `nundina.identity.v1`: `[U128 investor_id, String jurisdiction, U8 kyc]`. */
export function parseIdentity(d: Uint8Array): { investorId: Uint8Array; jurisdiction: string; kycLevel: number } | null {
  if (d.length !== 16 + 4 + 2 + 1) return null;
  if (new DataView(d.buffer, d.byteOffset).getUint32(16, true) !== 2) return null;
  return {
    investorId: d.slice(0, 16),
    jurisdiction: String.fromCharCode(d[20]!, d[21]!),
    kycLevel: d[22]!,
  };
}

/** `nundina.accreditation.v1`: `[Bool accredited, U8 method]`. */
export function parseAccreditation(d: Uint8Array): { accredited: boolean; method: number } | null {
  if (d.length !== 2 || d[0]! > 1) return null;
  return { accredited: d[0] === 1, method: d[1]! };
}
