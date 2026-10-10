//! credit-gate in the rails harness (#133, #134): every fill CPIs the gate's
//! `settle_check`, so every `Env` runs the real `credit_gate.so` (root
//! `anchor build`) and the real SAS program (the gate suite's pinned
//! fixture). The market, the attestations and the admissions go through
//! the programs' own instructions; only the lot back-dating moves the clock.

use anchor_lang::prelude::{Clock, Pubkey};
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use credit_gate::{
    accounts as gacc, instruction as gix, BlockEntry, BlockList, GateRoot, HolderLink, HolderLots,
    InvestorRecord, LotSource, MarketConfig, MarketPolicy, SAS_PROGRAM_ID,
};
use solana_keypair::Keypair;
use solana_signer::Signer;

use super::{Env, LOADER_V3, PROGRAM_ID};

pub const GATE_ID: Pubkey = credit_gate::ID;
/// Market seasoning (pilot value, CG-1).
pub const SEASONING: i64 = 7 * 86_400;
/// How far back a party's starting inventory was acquired: seasoned.
pub const INVENTORY_AGE: i64 = SEASONING + 86_400;
/// Attestation expiry: far past every test clock.
pub const ATTESTATION_EXPIRY: i64 = 400 * 86_400;
const SYSTEM: Pubkey = anchor_lang::system_program::ID;

pub fn gate_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &GATE_ID).0
}

fn sas_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &SAS_PROGRAM_ID).0
}

fn borsh_str(v: &mut Vec<u8>, s: &[u8]) {
    v.extend_from_slice(&(s.len() as u32).to_le_bytes());
    v.extend_from_slice(s);
}

/// The rails' settlement PDA (signs `settle_check`).
pub fn gate_settle_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"gate_settle"], &PROGRAM_ID).0
}

/// The SAS keys and schemas of the harness market.
pub struct Sas {
    pub cred_auth: Keypair,
    pub attester: Keypair,
    pub credential: Pubkey,
    pub id_schema: Pubkey,
    pub acc_schema: Pubkey,
    /// `nundina.nav.v1` (mark-engine ME-P1 NAV attestation).
    pub nav_schema: Pubkey,
    /// `nundina.redemption_request.v1` (queue-claim QC-P1).
    pub rr_schema: Pubkey,
}

impl Sas {
    pub fn attestation(&self, schema: &Pubkey, owner: &Pubkey) -> Pubkey {
        sas_pda(&[
            b"attestation",
            self.credential.as_ref(),
            schema.as_ref(),
            owner.as_ref(),
        ])
    }
}

/// `nundina.identity.v1` data: investor_id, US, KYC 2.
pub fn identity_data(investor_id: [u8; 16]) -> Vec<u8> {
    let mut d = investor_id.to_vec();
    borsh_str(&mut d, b"US");
    d.push(2);
    d
}

/// A non-zero investor id unique to `wallet`.
pub fn investor_of(wallet: &Pubkey) -> [u8; 16] {
    let mut id: [u8; 16] = wallet.to_bytes()[..16].try_into().unwrap();
    id[0] |= 1;
    id
}

impl Env {
    /// Load credit-gate + SAS, seed the gate root and the SAS credential and
    /// schemas. Called by `Env::new`; the market itself is per asset mint
    /// ([`Env::open_gate_market`]).
    pub(super) fn load_gate(&mut self) -> Sas {
        let gate_so = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/deploy/credit_gate.so"
        );
        let bytes = std::fs::read(gate_so)
            .unwrap_or_else(|e| panic!("{gate_so}: {e} — run `anchor build` at the repo root"));
        self.svm.add_program(GATE_ID, &bytes).expect("load gate");
        let sas_so = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../programs/credit-gate/tests-svm/fixtures/sas.so"
        );
        self.svm
            .add_program(SAS_PROGRAM_ID, &std::fs::read(sas_so).expect("sas fixture"))
            .expect("load SAS");

        // The rails authority doubles as the gate's upgrade + root authority.
        let auth = self.authority.insecure_clone();
        let (programdata, _) = Pubkey::find_program_address(&[GATE_ID.as_ref()], &LOADER_V3);
        let mut pd = self
            .svm
            .get_account(&programdata)
            .expect("gate programdata");
        pd.data[12] = 1;
        pd.data[13..45].copy_from_slice(auth.pubkey().as_ref());
        self.svm.set_account(programdata, pd).unwrap();
        let root = Instruction {
            program_id: GATE_ID,
            accounts: gacc::InitializeRoot {
                root: gate_pda(&[GateRoot::SEED]),
                upgrade_authority: auth.pubkey(),
                program: GATE_ID,
                program_data: programdata,
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: gix::InitializeRoot {
                authority: auth.pubkey(),
            }
            .data(),
        };
        self.send(&[root], &[&auth]).expect("gate root");

        let (cred_auth, attester) = (self.funded(), self.funded());
        let credential = sas_pda(&[b"credential", cred_auth.pubkey().as_ref(), b"nundina"]);
        let schema = |name: &[u8]| sas_pda(&[b"schema", credential.as_ref(), name, &[1]]);
        let sas = Sas {
            id_schema: schema(b"nundina.identity.v1"),
            acc_schema: schema(b"nundina.accreditation.v1"),
            nav_schema: schema(b"nundina.nav.v1"),
            rr_schema: schema(b"nundina.redemption_request.v1"),
            cred_auth,
            attester,
            credential,
        };
        let mut data = vec![0u8];
        borsh_str(&mut data, b"nundina");
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(sas.attester.pubkey().as_ref());
        let ca = sas.cred_auth.insecure_clone();
        let ix = Instruction {
            program_id: SAS_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(ca.pubkey(), true),
                AccountMeta::new(credential, false),
                AccountMeta::new_readonly(ca.pubkey(), true),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
            data,
        };
        self.send(&[ix], &[&ca]).expect("SAS credential");
        for (name, layout, fields) in [
            (
                &b"nundina.identity.v1"[..],
                &[4u8, 12, 0][..],
                &["investor_id", "jurisdiction", "kyc_level"][..],
            ),
            (
                &b"nundina.accreditation.v1"[..],
                &[10u8, 0][..],
                &["accredited", "method"][..],
            ),
            (
                &b"nundina.nav.v1"[..],
                &[13u8, 4, 8, 2, 13][..],
                &[
                    "asset_mint",
                    "nav_pico",
                    "computed_ts",
                    "model_version",
                    "inputs_hash",
                ][..],
            ),
            (
                &b"nundina.redemption_request.v1"[..],
                &[3u8, 3, 8][..],
                &["request_id", "qty", "window_date"][..],
            ),
        ] {
            let mut data = vec![1u8];
            borsh_str(&mut data, name);
            borsh_str(&mut data, b"NUNDINA pilot schema");
            borsh_str(&mut data, layout);
            data.extend_from_slice(&(fields.len() as u32).to_le_bytes());
            for f in fields {
                borsh_str(&mut data, f.as_bytes());
            }
            let ix = Instruction {
                program_id: SAS_PROGRAM_ID,
                accounts: vec![
                    AccountMeta::new(ca.pubkey(), true),
                    AccountMeta::new_readonly(ca.pubkey(), true),
                    AccountMeta::new_readonly(credential, false),
                    AccountMeta::new(schema(name), false),
                    AccountMeta::new_readonly(SYSTEM, false),
                ],
                data,
            };
            self.send(&[ix], &[&ca]).expect("SAS schema");
        }
        sas
    }

    /// Gate market for `self.asset_mint`: config (seasoning 7 d, cap 100,
    /// the harness attester), policy (US, KYC ≥ 1, `require_accreditation`)
    /// and an (empty) block-list.
    pub fn open_gate_market_with(&mut self, require_accreditation: bool) {
        let auth = self.authority.insecure_clone();
        let mint = self.asset_mint;
        let config = self.gate_config();
        let ixs = [
            Instruction {
                program_id: GATE_ID,
                accounts: gacc::InitializeConfig {
                    config,
                    root: gate_pda(&[GateRoot::SEED]),
                    mint,
                    authority: auth.pubkey(),
                    system_program: SYSTEM,
                }
                .to_account_metas(None),
                data: gix::InitializeConfig {
                    policy_class: 1,
                    holder_cap: 100,
                    seasoning_seconds: SEASONING,
                    settlement_horizon_seconds: 86_400,
                    attesters: vec![self.sas.attester.pubkey()],
                    identity_schema: self.sas.id_schema,
                    accreditation_schema: self.sas.acc_schema,
                }
                .data(),
            },
            Instruction {
                program_id: GATE_ID,
                accounts: gacc::InitializePolicy {
                    config,
                    policy: self.gate_policy(),
                    identity_schema: self.sas.id_schema,
                    accreditation_schema: self.sas.acc_schema,
                    mint,
                    gate_config_authority: auth.pubkey(),
                    system_program: SYSTEM,
                }
                .to_account_metas(None),
                data: gix::InitializePolicy {
                    credential: self.sas.credential,
                    jurisdictions: vec![*b"US"],
                    min_kyc_level: 1,
                    require_accreditation,
                }
                .data(),
            },
            Instruction {
                program_id: GATE_ID,
                accounts: gacc::InitializeBlocklist {
                    config,
                    blocklist: gate_pda(&[BlockList::SEED, mint.as_ref()]),
                    mint,
                    gate_config_authority: auth.pubkey(),
                    system_program: SYSTEM,
                }
                .to_account_metas(None),
                data: gix::InitializeBlocklist {}.data(),
            },
        ];
        self.send(&ixs, &[&auth]).expect("gate market");
    }

    pub fn open_gate_market(&mut self) {
        self.open_gate_market_with(true);
    }

    pub fn gate_config(&self) -> Pubkey {
        gate_pda(&[MarketConfig::SEED, self.asset_mint.as_ref()])
    }
    pub fn gate_policy(&self) -> Pubkey {
        gate_pda(&[MarketPolicy::SEED, self.asset_mint.as_ref()])
    }
    pub fn block_entry(&self, wallet: &Pubkey) -> Pubkey {
        gate_pda(&[BlockEntry::SEED, self.asset_mint.as_ref(), wallet.as_ref()])
    }
    pub fn lots_pda(&self, wallet: &Pubkey) -> Pubkey {
        gate_pda(&[HolderLots::SEED, self.asset_mint.as_ref(), wallet.as_ref()])
    }
    pub fn link_pda(&self, wallet: &Pubkey) -> Pubkey {
        gate_pda(&[HolderLink::SEED, self.asset_mint.as_ref(), wallet.as_ref()])
    }
    pub fn identity_att(&self, wallet: &Pubkey) -> Pubkey {
        self.sas.attestation(&self.sas.id_schema, wallet)
    }
    pub fn accreditation_att(&self, wallet: &Pubkey) -> Pubkey {
        self.sas.attestation(&self.sas.acc_schema, wallet)
    }

    pub fn lots(&self, wallet: &Pubkey) -> HolderLots {
        let a = self
            .svm
            .get_account(&self.lots_pda(wallet))
            .expect("ledger");
        HolderLots::try_deserialize(&mut &a.data[..]).unwrap()
    }

    /// SAS issues `data` under `schema` for `owner` (the harness attester).
    pub fn sas_attest(&mut self, schema: Pubkey, owner: &Pubkey, data: Vec<u8>, expiry: i64) {
        let att = self.sas.attestation(&schema, owner);
        let mut d = vec![6u8];
        d.extend_from_slice(owner.as_ref());
        borsh_str(&mut d, &data);
        d.extend_from_slice(&expiry.to_le_bytes());
        let a = self.sas.attester.insecure_clone();
        let ix = Instruction {
            program_id: SAS_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(a.pubkey(), true),
                AccountMeta::new_readonly(a.pubkey(), true),
                AccountMeta::new_readonly(self.sas.credential, false),
                AccountMeta::new_readonly(schema, false),
                AccountMeta::new(att, false),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
            data: d,
        };
        self.send(&[ix], &[&a]).expect("SAS attest");
    }

    /// SAS `CloseAttestation` (7): revocation.
    pub fn sas_close(&mut self, att: Pubkey) {
        let a = self.sas.attester.insecure_clone();
        let ix = Instruction {
            program_id: SAS_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(a.pubkey(), true),
                AccountMeta::new_readonly(a.pubkey(), true),
                AccountMeta::new_readonly(self.sas.credential, false),
                AccountMeta::new(att, false),
                AccountMeta::new_readonly(sas_pda(&[b"__event_authority"]), false),
                AccountMeta::new_readonly(SYSTEM, false),
                AccountMeta::new_readonly(SAS_PROGRAM_ID, false),
            ],
            data: vec![7],
        };
        self.send(&[ix], &[&a]).expect("SAS close");
    }

    /// The gate's `validate` for `wallet` (anyone pays: the authority here).
    pub fn validate(&mut self, wallet: &Pubkey) -> super::Res {
        self.validate_as(wallet, investor_of(wallet))
    }

    /// `validate` for `wallet`, whose identity attestation names `investor_id`.
    pub fn validate_as(&mut self, wallet: &Pubkey, investor_id: [u8; 16]) -> super::Res {
        let auth = self.authority.insecure_clone();
        let mint = self.asset_mint;
        let ix = Instruction {
            program_id: GATE_ID,
            accounts: gacc::Validate {
                config: self.gate_config(),
                policy: self.gate_policy(),
                block_entry: self.block_entry(wallet),
                identity_attestation: self.identity_att(wallet),
                accreditation_attestation: self.accreditation_att(wallet),
                holder_link: self.link_pda(wallet),
                investor: gate_pda(&[InvestorRecord::SEED, mint.as_ref(), &investor_id]),
                lots: self.lots_pda(wallet),
                mint,
                payer: auth.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: gix::Validate { owner: *wallet }.data(),
        };
        self.send(&[ix], &[&auth])
    }

    /// Attest (identity + accreditation) and admit `wallet`, then record
    /// `seasoned` units of inventory acquired `INVENTORY_AGE` ago.
    pub fn admit(&mut self, wallet: &Pubkey, seasoned: u64) {
        self.admit_as(wallet, true);
        if seasoned > 0 {
            let now = self.now();
            self.append_lot_at(wallet, seasoned, now - INVENTORY_AGE);
        }
    }

    /// Attest `wallet` (accredited or not) and run `validate`.
    pub fn admit_as(&mut self, wallet: &Pubkey, accredited: bool) {
        self.admit_investor(wallet, investor_of(wallet), accredited);
    }

    /// Attest and admit `wallet` as one of investor `investor_id`'s wallets
    /// (the gate links any number of wallets to one investor).
    pub fn admit_investor(&mut self, wallet: &Pubkey, investor_id: [u8; 16], accredited: bool) {
        let (id, acc) = (self.sas.id_schema, self.sas.acc_schema);
        self.sas_attest(id, wallet, identity_data(investor_id), ATTESTATION_EXPIRY);
        self.sas_attest(
            acc,
            wallet,
            vec![u8::from(accredited), 1],
            ATTESTATION_EXPIRY,
        );
        self.validate_as(wallet, investor_id).expect("validate");
        assert!(self.exists(&self.link_pda(wallet)), "admitted");
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    /// The gate authority records a lot for `wallet` acquired at `at`
    /// (`append_lot` stamps the clock, so the clock visits `at`).
    pub fn append_lot_at(&mut self, wallet: &Pubkey, qty: u64, at: i64) {
        let now = self.now();
        self.set_time(at);
        let auth = self.authority.insecure_clone();
        let mint = self.asset_mint;
        let ix = Instruction {
            program_id: GATE_ID,
            accounts: gacc::AppendLot {
                config: self.gate_config(),
                lots: self.lots_pda(wallet),
                block_entry: self.block_entry(wallet),
                mint,
                gate_config_authority: auth.pubkey(),
            }
            .to_account_metas(None),
            data: gix::AppendLot {
                qty,
                source: LotSource::PrimaryMint,
                affiliate: false,
                exemption_path: 0,
            }
            .data(),
        };
        self.send(&[ix], &[&auth]).expect("append_lot");
        self.set_time(now);
    }

    /// The market's live policy (CG-P4 count included).
    pub fn policy_state(&self) -> MarketPolicy {
        let a = self.svm.get_account(&self.gate_policy()).expect("policy");
        MarketPolicy::try_deserialize(&mut &a.data[..]).unwrap()
    }

    /// `update_config(holder_cap)` by the gate authority (CG-P4).
    pub fn set_holder_cap(&mut self, cap: u32) {
        let auth = self.authority.insecure_clone();
        let ix = Instruction {
            program_id: GATE_ID,
            accounts: gacc::UpdateConfig {
                config: self.gate_config(),
                mint: self.asset_mint,
                gate_config_authority: auth.pubkey(),
            }
            .to_account_metas(None),
            data: gix::UpdateConfig {
                policy_class: None,
                holder_cap: Some(cap),
                seasoning_seconds: None,
                settlement_horizon_seconds: None,
                attester_add: None,
                attester_remove: None,
                blocklist_set: None,
            }
            .data(),
        };
        self.send(&[ix], &[&auth]).expect("update_config");
    }

    /// `update_compliance` by the gate authority (#202 compliance tail).
    pub fn update_compliance(&mut self, update: credit_gate::ComplianceUpdate) -> super::Res {
        let auth = self.authority.insecure_clone();
        let ix = Instruction {
            program_id: GATE_ID,
            accounts: gacc::UpdateConfig {
                config: self.gate_config(),
                mint: self.asset_mint,
                gate_config_authority: auth.pubkey(),
            }
            .to_account_metas(None),
            data: gix::UpdateCompliance { update }.data(),
        };
        self.send(&[ix], &[&auth])
    }

    /// Put `wallet` on the market block-list (CG-P5).
    pub fn block(&mut self, wallet: &Pubkey) {
        let auth = self.authority.insecure_clone();
        let mint = self.asset_mint;
        let ix = Instruction {
            program_id: GATE_ID,
            accounts: gacc::BlockWallet {
                config: self.gate_config(),
                blocklist: gate_pda(&[BlockList::SEED, mint.as_ref()]),
                entry: self.block_entry(wallet),
                mint,
                gate_config_authority: auth.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: gix::BlockWallet {
                wallet: *wallet,
                reason_hash: [0xbb; 32],
            }
            .data(),
        };
        self.send(&[ix], &[&auth]).expect("block_wallet");
    }
}
