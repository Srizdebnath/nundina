//! QC-P1: the SAS-attested redemption request (#202).
//!
//! When a queue's [`QueueConfig`](crate::QueueConfig) sets `require_sas`,
//! `request_redemption` only opens a request against a real Solana
//! Attestation Service attestation the issuer / transfer-agent bridge
//! signed for it. A minimal, strict copy of the SAS account decode that
//! credit-gate uses (`credit-gate/src/eligibility.rs`), kept here so
//! queue-claim does not depend on the gate program.
//!
//! The attestation counts only if it is owned by the SAS program, carries
//! SAS's attestation discriminator, sits at its canonical PDA
//! `["attestation", credential, schema, nonce]` with `nonce` = the holder
//! wallet, is under the queue's credential and `redemption_request` schema,
//! was signed by the queue's SAS attester, has not expired (`expiry == 0`
//! = SAS "never expires"), and its data names this exact request.
//!
//! **Schema** `nundina.redemption_request.v1` (SAS layout `[U64, U64,
//! I64]`): `request_id` (the queue's next request id — one attestation
//! opens one request, so it cannot be replayed), `qty` (base units) and
//! `window_date`.

use anchor_lang::prelude::*;

/// The Solana Attestation Service program (same id on devnet and mainnet).
pub const SAS_PROGRAM_ID: Pubkey = pubkey!("22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG");
/// SAS account discriminator of an `Attestation`.
pub const SAS_ATTESTATION_DISCRIMINATOR: u8 = 2;
/// SAS PDA seed of an attestation.
pub const SAS_ATTESTATION_SEED: &[u8] = b"attestation";

/// A decoded SAS attestation account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SasAttestation {
    pub nonce: Pubkey,
    pub credential: Pubkey,
    pub schema: Pubkey,
    pub data: Vec<u8>,
    pub signer: Pubkey,
    pub expiry: i64,
    pub token_account: Pubkey,
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if b.len() < n {
        return None;
    }
    let (h, t) = b.split_at(n);
    *b = t;
    Some(h)
}

fn key(b: &mut &[u8]) -> Option<Pubkey> {
    Some(Pubkey::new_from_array(take(b, 32)?.try_into().ok()?))
}

fn u64le(b: &mut &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(take(b, 8)?.try_into().ok()?))
}

impl SasAttestation {
    /// Strict decode of SAS's account bytes: discriminator, then
    /// `nonce | credential | schema | u32-len data | signer | i64 expiry |
    /// token_account`, and nothing after. `None` on any mismatch.
    pub fn parse(bytes: &[u8]) -> Option<SasAttestation> {
        let mut b = bytes;
        if *take(&mut b, 1)?.first()? != SAS_ATTESTATION_DISCRIMINATOR {
            return None;
        }
        let (nonce, credential, schema) = (key(&mut b)?, key(&mut b)?, key(&mut b)?);
        let len = u32::from_le_bytes(take(&mut b, 4)?.try_into().ok()?) as usize;
        let data = take(&mut b, len)?.to_vec();
        let signer = key(&mut b)?;
        let expiry = i64::from_le_bytes(take(&mut b, 8)?.try_into().ok()?);
        let token_account = key(&mut b)?;
        b.is_empty().then_some(SasAttestation {
            nonce,
            credential,
            schema,
            data,
            signer,
            expiry,
            token_account,
        })
    }

    /// The canonical SAS address of this attestation.
    pub fn address(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[
                SAS_ATTESTATION_SEED,
                self.credential.as_ref(),
                self.schema.as_ref(),
                self.nonce.as_ref(),
            ],
            &SAS_PROGRAM_ID,
        )
        .0
    }
}

/// `nundina.redemption_request.v1` (SAS layout `[U64, U64, I64]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedemptionRequestData {
    pub request_id: u64,
    pub qty: u64,
    pub window_date: i64,
}

impl RedemptionRequestData {
    pub fn parse(data: &[u8]) -> Option<RedemptionRequestData> {
        let mut b = data;
        let request_id = u64le(&mut b)?;
        let qty = u64le(&mut b)?;
        let window_date = u64le(&mut b)? as i64;
        b.is_empty().then_some(RedemptionRequestData {
            request_id,
            qty,
            window_date,
        })
    }

    /// SAS's encoding of the record (what the attester writes).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(24);
        v.extend_from_slice(&self.request_id.to_le_bytes());
        v.extend_from_slice(&self.qty.to_le_bytes());
        v.extend_from_slice(&self.window_date.to_le_bytes());
        v
    }
}

/// Why an attestation does not open the request (all map to
/// `QcError::BadRedemptionAttestation`; the variant is logged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SasReject {
    NotSasOwned,
    Malformed,
    NotCanonical,
    WrongHolder,
    WrongCredential,
    WrongSchema,
    WrongAttester,
    Expired,
    WrongRequest,
}

/// The QC-P1 check over decoded parts (the handler supplies the account
/// owner, key and bytes).
#[allow(clippy::too_many_arguments)]
pub fn check_request_attestation(
    owner: &Pubkey,
    address: &Pubkey,
    bytes: &[u8],
    credential: &Pubkey,
    schema: &Pubkey,
    attester: &Pubkey,
    holder: &Pubkey,
    expected: &RedemptionRequestData,
    now: i64,
) -> core::result::Result<(), SasReject> {
    if *owner != SAS_PROGRAM_ID {
        return Err(SasReject::NotSasOwned);
    }
    let a = SasAttestation::parse(bytes).ok_or(SasReject::Malformed)?;
    if a.nonce != *holder {
        return Err(SasReject::WrongHolder);
    }
    if a.credential != *credential {
        return Err(SasReject::WrongCredential);
    }
    if a.schema != *schema {
        return Err(SasReject::WrongSchema);
    }
    if a.address() != *address {
        return Err(SasReject::NotCanonical);
    }
    if a.signer != *attester {
        return Err(SasReject::WrongAttester);
    }
    if a.expiry != 0 && a.expiry <= now {
        return Err(SasReject::Expired);
    }
    match RedemptionRequestData::parse(&a.data) {
        Some(d) if d == *expected => Ok(()),
        _ => Err(SasReject::WrongRequest),
    }
}

/// SAS's byte layout of an attestation account (tests and fixtures).
pub fn encode_attestation(a: &SasAttestation) -> Vec<u8> {
    let mut v = vec![SAS_ATTESTATION_DISCRIMINATOR];
    v.extend_from_slice(a.nonce.as_ref());
    v.extend_from_slice(a.credential.as_ref());
    v.extend_from_slice(a.schema.as_ref());
    v.extend_from_slice(&(a.data.len() as u32).to_le_bytes());
    v.extend_from_slice(&a.data);
    v.extend_from_slice(a.signer.as_ref());
    v.extend_from_slice(&a.expiry.to_le_bytes());
    v.extend_from_slice(a.token_account.as_ref());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        credential: Pubkey,
        schema: Pubkey,
        attester: Pubkey,
        holder: Pubkey,
        req: RedemptionRequestData,
    }

    fn fx() -> Fx {
        Fx {
            credential: Pubkey::new_unique(),
            schema: Pubkey::new_unique(),
            attester: Pubkey::new_unique(),
            holder: Pubkey::new_unique(),
            req: RedemptionRequestData {
                request_id: 3,
                qty: 10_000_000,
                window_date: 1_800_000_000,
            },
        }
    }

    fn att(f: &Fx) -> SasAttestation {
        SasAttestation {
            nonce: f.holder,
            credential: f.credential,
            schema: f.schema,
            data: f.req.encode(),
            signer: f.attester,
            expiry: 0,
            token_account: Pubkey::default(),
        }
    }

    fn check(
        f: &Fx,
        a: &SasAttestation,
        owner: Pubkey,
        now: i64,
    ) -> core::result::Result<(), SasReject> {
        check_request_attestation(
            &owner,
            &a.address(),
            &encode_attestation(a),
            &f.credential,
            &f.schema,
            &f.attester,
            &f.holder,
            &f.req,
            now,
        )
    }

    #[test]
    fn round_trips_and_accepts_the_attested_request() {
        let f = fx();
        let a = att(&f);
        assert_eq!(
            SasAttestation::parse(&encode_attestation(&a)),
            Some(a.clone())
        );
        assert_eq!(RedemptionRequestData::parse(&f.req.encode()), Some(f.req));
        assert_eq!(check(&f, &a, SAS_PROGRAM_ID, 0), Ok(()));
    }

    #[test]
    fn rejects_every_mismatch() {
        let f = fx();
        let a = att(&f);
        assert_eq!(
            check(&f, &a, Pubkey::new_unique(), 0),
            Err(SasReject::NotSasOwned)
        );
        let mut b = a.clone();
        b.signer = Pubkey::new_unique();
        assert_eq!(
            check(&f, &b, SAS_PROGRAM_ID, 0),
            Err(SasReject::WrongAttester)
        );
        let mut b = a.clone();
        b.nonce = Pubkey::new_unique();
        assert_eq!(
            check(&f, &b, SAS_PROGRAM_ID, 0),
            Err(SasReject::WrongHolder)
        );
        let mut b = a.clone();
        b.schema = Pubkey::new_unique();
        assert_eq!(
            check(&f, &b, SAS_PROGRAM_ID, 0),
            Err(SasReject::WrongSchema)
        );
        let mut b = a.clone();
        b.credential = Pubkey::new_unique();
        assert_eq!(
            check(&f, &b, SAS_PROGRAM_ID, 0),
            Err(SasReject::WrongCredential)
        );
        let mut b = a.clone();
        b.expiry = 100;
        assert_eq!(check(&f, &b, SAS_PROGRAM_ID, 100), Err(SasReject::Expired));
        assert_eq!(check(&f, &b, SAS_PROGRAM_ID, 99), Ok(()));
        // Replay: the attestation for request 3 does not open request 4.
        let mut b = a.clone();
        b.data = RedemptionRequestData {
            request_id: 4,
            ..f.req
        }
        .encode();
        assert_eq!(
            check(&f, &b, SAS_PROGRAM_ID, 0),
            Err(SasReject::WrongRequest)
        );
        // A non-canonical address (an account at another key).
        let r = check_request_attestation(
            &SAS_PROGRAM_ID,
            &Pubkey::new_unique(),
            &encode_attestation(&a),
            &f.credential,
            &f.schema,
            &f.attester,
            &f.holder,
            &f.req,
            0,
        );
        assert_eq!(r, Err(SasReject::NotCanonical));
        // Trailing bytes / truncation.
        let mut bytes = encode_attestation(&a);
        bytes.push(0);
        assert_eq!(SasAttestation::parse(&bytes), None);
        assert_eq!(RedemptionRequestData::parse(&[0; 23]), None);
    }
}
