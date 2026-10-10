//! #122 end to end, against the REAL programs (fixtures/README.md):
//! SAS issues the attestations, credit-gate `validate`s them, and the real
//! Token ACL program thaws (or refuses to thaw) a frozen-by-default
//! Token-2022 account by CPI into the gate's sRFC37 hook, resolving the
//! gate's extra-account-meta TLV itself.
//!
//! Covered: approval → permissionless thaw; every typed rejection
//! (block-list, unknown signer, wrong schema, expiry inside the horizon,
//! jurisdiction, KYC, accreditation, governor cap); one investor's second
//! wallet counted once; release → headroom; revocation by SAS close →
//! anyone may freeze; an eligible holder can't be frozen; the hook refuses
//! calls from outside Token ACL.

mod common;

use anchor_lang::prelude::{Clock, Pubkey};
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{AccountDeserialize, AnchorDeserialize, Discriminator};
use anchor_lang::{InstructionData, ToAccountMetas};
use base64::Engine;
use common::{deploy, root_pda, seed_root, PROGRAM_ID};
use credit_gate::{
    accounts as acc, associated_token_2022, field_group_ext, instruction as ix, AffiliateRecord,
    AffiliateSet, BlockEntry, BlockList, Compliance, ComplianceUpdate, ConfigUpdated,
    ExemptionClass, GateError, GateReject, HolderLink, HolderLots, InvestorRecord, LotSource,
    MarketConfig, MarketPolicy, SeizureRecorded, ValidateDecision, SAS_PROGRAM_ID, TOKEN_2022_ID,
    TOKEN_ACL_ID,
};
use litesvm::LiteSVM;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

const DAY: i64 = 86_400;
const NOW: i64 = 1_800_000_000;
const SYSTEM: Pubkey = anchor_lang::system_program::ID;

// ── encoders for the two external programs ────────────────────────────

fn borsh_str(v: &mut Vec<u8>, s: &[u8]) {
    v.extend_from_slice(&(s.len() as u32).to_le_bytes());
    v.extend_from_slice(s);
}

fn pda(seeds: &[&[u8]], program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(seeds, program).0
}

/// SAS schema types (program `state/schema.rs`).
const U8: u8 = 0;
const U128: u8 = 4;
const BOOL: u8 = 10;
const STRING: u8 = 12;

fn identity_data(investor: u8, jurisdiction: &[u8], kyc: u8) -> Vec<u8> {
    let mut d = vec![investor; 16];
    borsh_str(&mut d, jurisdiction);
    d.push(kyc);
    d
}

/// Token-2022 account state byte (base layout offset 108).
const STATE_OFFSET: usize = 108;
const INITIALIZED: u8 = 1;
const FROZEN: u8 = 2;

// ── harness ───────────────────────────────────────────────────────────

struct Env {
    svm: LiteSVM,
    gov: Keypair,
    cred_auth: Keypair,
    attester: Keypair,
    credential: Pubkey,
    id_schema: Pubkey,
    acc_schema: Pubkey,
    mint: Keypair,
}

impl Env {
    /// Gate + SAS + Token ACL loaded; credential and two schemas created in
    /// SAS; market configured (cap `holder_cap`, attester = `attester`),
    /// policy US-only, KYC ≥ 1, accreditation required; NPCS-shaped mint
    /// (DefaultAccountState = Frozen) handed to Token ACL with permissionless
    /// thaw and freeze routed to the gate.
    fn new(holder_cap: u32) -> Env {
        let gov = Keypair::new();
        let mut svm = deploy(&gov);
        seed_root(&mut svm, &gov);
        let fx = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/");
        svm.add_program(
            SAS_PROGRAM_ID,
            &std::fs::read(format!("{fx}sas.so")).unwrap(),
        )
        .unwrap();
        svm.add_program(
            TOKEN_ACL_ID,
            &std::fs::read(format!("{fx}token_acl.so")).unwrap(),
        )
        .unwrap();
        let mut c: Clock = svm.get_sysvar();
        c.unix_timestamp = NOW;
        svm.set_sysvar(&c);

        let (cred_auth, attester, mint) = (Keypair::new(), Keypair::new(), Keypair::new());
        for k in [&cred_auth, &attester] {
            svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        }
        let credential = pda(
            &[b"credential", cred_auth.pubkey().as_ref(), b"nundina"],
            &SAS_PROGRAM_ID,
        );
        let schema = |name: &[u8]| {
            pda(
                &[b"schema", credential.as_ref(), name, &[1]],
                &SAS_PROGRAM_ID,
            )
        };
        let mut env = Env {
            svm,
            gov,
            cred_auth,
            attester,
            credential,
            id_schema: schema(b"nundina.identity.v1"),
            acc_schema: schema(b"nundina.accreditation.v1"),
            mint,
        };
        env.sas_credential();
        env.sas_schema(
            b"nundina.identity.v1",
            &[U128, STRING, U8],
            &["investor_id", "jurisdiction", "kyc_level"],
        );
        env.sas_schema(
            b"nundina.accreditation.v1",
            &[BOOL, U8],
            &["accredited", "method"],
        );
        env.init_market(holder_cap);
        env.init_mint_under_token_acl();
        env
    }

    fn send(
        &mut self,
        ixs: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Vec<String>, (u32, Vec<String>)> {
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&signers[0].pubkey()),
            signers,
            self.svm.latest_blockhash(),
        );
        let out = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        match out {
            Ok(m) => Ok(m.logs),
            Err(f) => match f.err {
                TransactionError::InstructionError(_, InstructionError::Custom(c)) => {
                    Err((c, f.meta.logs))
                }
                other => panic!("unexpected failure {other:?} | {:?}", f.meta.logs),
            },
        }
    }

    fn ok(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Vec<String> {
        self.send(ixs, signers)
            .unwrap_or_else(|(c, l)| panic!("error {c}: {l:#?}"))
    }

    fn funded(&mut self) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        k
    }

    // SAS -------------------------------------------------------------

    fn sas_credential(&mut self) {
        let mut data = vec![0u8];
        borsh_str(&mut data, b"nundina");
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(self.attester.pubkey().as_ref());
        let ix = Instruction {
            program_id: SAS_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.cred_auth.pubkey(), true),
                AccountMeta::new(self.credential, false),
                AccountMeta::new_readonly(self.cred_auth.pubkey(), true),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
            data,
        };
        let a = self.cred_auth.insecure_clone();
        self.ok(&[ix], &[&a]);
    }

    fn sas_schema(&mut self, name: &[u8], layout: &[u8], fields: &[&str]) {
        let schema = pda(
            &[b"schema", self.credential.as_ref(), name, &[1]],
            &SAS_PROGRAM_ID,
        );
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
                AccountMeta::new(self.cred_auth.pubkey(), true),
                AccountMeta::new_readonly(self.cred_auth.pubkey(), true),
                AccountMeta::new_readonly(self.credential, false),
                AccountMeta::new(schema, false),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
            data,
        };
        let a = self.cred_auth.insecure_clone();
        self.ok(&[ix], &[&a]);
    }

    fn attestation_address(&self, schema: &Pubkey, owner: &Pubkey) -> Pubkey {
        pda(
            &[
                b"attestation",
                self.credential.as_ref(),
                schema.as_ref(),
                owner.as_ref(),
            ],
            &SAS_PROGRAM_ID,
        )
    }

    /// Issue `data` under `schema` for `owner`, signed by `signer` (must be
    /// an authorized signer of the credential for SAS to accept it).
    fn attest(&mut self, schema: Pubkey, owner: Pubkey, data: Vec<u8>, expiry: i64) -> Pubkey {
        let att = self.attestation_address(&schema, &owner);
        let mut d = vec![6u8];
        d.extend_from_slice(owner.as_ref());
        borsh_str(&mut d, &data);
        d.extend_from_slice(&expiry.to_le_bytes());
        let ix = Instruction {
            program_id: SAS_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.attester.pubkey(), true),
                AccountMeta::new_readonly(self.attester.pubkey(), true),
                AccountMeta::new_readonly(self.credential, false),
                AccountMeta::new_readonly(schema, false),
                AccountMeta::new(att, false),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
            data: d,
        };
        let a = self.attester.insecure_clone();
        self.ok(&[ix], &[&a]);
        att
    }

    /// Identity (US, KYC 2) + accreditation (accredited) for `owner`.
    fn attest_good(&mut self, owner: Pubkey, investor: u8) -> (Pubkey, Pubkey) {
        let (id, ac) = (self.id_schema, self.acc_schema);
        (
            self.attest(id, owner, identity_data(investor, b"US", 2), NOW + 90 * DAY),
            self.attest(ac, owner, vec![1, 1], NOW + 90 * DAY),
        )
    }

    /// Revoke in SAS: close the attestation (SAS `CloseAttestation`, 7).
    fn sas_close(&mut self, att: Pubkey) {
        let event_authority = pda(&[b"__event_authority"], &SAS_PROGRAM_ID);
        let ix = Instruction {
            program_id: SAS_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.attester.pubkey(), true),
                AccountMeta::new_readonly(self.attester.pubkey(), true),
                AccountMeta::new_readonly(self.credential, false),
                AccountMeta::new(att, false),
                AccountMeta::new_readonly(event_authority, false),
                AccountMeta::new_readonly(SYSTEM, false),
                AccountMeta::new_readonly(SAS_PROGRAM_ID, false),
            ],
            data: vec![7],
        };
        let a = self.attester.insecure_clone();
        self.ok(&[ix], &[&a]);
        assert!(self.svm.get_account(&att).is_none_or(|a| a.data.is_empty()));
    }

    // gate ------------------------------------------------------------

    fn config_pda(&self) -> Pubkey {
        pda(
            &[MarketConfig::SEED, self.mint.pubkey().as_ref()],
            &PROGRAM_ID,
        )
    }
    fn policy_pda(&self) -> Pubkey {
        pda(
            &[MarketPolicy::SEED, self.mint.pubkey().as_ref()],
            &PROGRAM_ID,
        )
    }
    fn link_pda(&self, owner: &Pubkey) -> Pubkey {
        pda(
            &[
                HolderLink::SEED,
                self.mint.pubkey().as_ref(),
                owner.as_ref(),
            ],
            &PROGRAM_ID,
        )
    }
    fn entry_pda(&self, owner: &Pubkey) -> Pubkey {
        pda(
            &[
                BlockEntry::SEED,
                self.mint.pubkey().as_ref(),
                owner.as_ref(),
            ],
            &PROGRAM_ID,
        )
    }
    fn investor_pda(&self, id: u8) -> Pubkey {
        pda(
            &[InvestorRecord::SEED, self.mint.pubkey().as_ref(), &[id; 16]],
            &PROGRAM_ID,
        )
    }

    fn init_market(&mut self, holder_cap: u32) {
        let mint = self.mint.pubkey();
        let gov = self.gov.insecure_clone();
        let init = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeConfig {
                config: self.config_pda(),
                root: root_pda(),
                mint,
                authority: gov.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: ix::InitializeConfig {
                policy_class: 1,
                holder_cap,
                seasoning_seconds: 7 * DAY,
                settlement_horizon_seconds: DAY,
                attesters: vec![self.attester.pubkey()],
                identity_schema: self.id_schema,
                accreditation_schema: self.acc_schema,
            }
            .data(),
        };
        let policy = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializePolicy {
                config: self.config_pda(),
                policy: self.policy_pda(),
                identity_schema: self.id_schema,
                accreditation_schema: self.acc_schema,
                mint,
                gate_config_authority: gov.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: ix::InitializePolicy {
                credential: self.credential,
                jurisdictions: vec![*b"US"],
                min_kyc_level: 1,
                require_accreditation: true,
            }
            .data(),
        };
        let blocklist = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeBlocklist {
                config: self.config_pda(),
                blocklist: BlockList::address(&mint),
                mint,
                gate_config_authority: gov.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: ix::InitializeBlocklist {}.data(),
        };
        let metas = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeTokenAclMetas {
                config: self.config_pda(),
                thaw_metas: pda(&[b"thaw_extra_account_metas", mint.as_ref()], &PROGRAM_ID),
                freeze_metas: pda(&[b"freeze_extra_account_metas", mint.as_ref()], &PROGRAM_ID),
                mint,
                gate_config_authority: gov.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: ix::InitializeTokenAclMetas {}.data(),
        };
        self.ok(&[init], &[&gov]);
        self.ok(&[policy, blocklist, metas], &[&gov]);
    }

    fn mint_config(&self) -> Pubkey {
        pda(
            &[b"MINT_CONFIG", self.mint.pubkey().as_ref()],
            &TOKEN_ACL_ID,
        )
    }

    /// Token-2022 mint with DefaultAccountState = Frozen; Token ACL takes
    /// the freeze authority and routes permissionless thaw/freeze to us.
    fn init_mint_under_token_acl(&mut self) {
        let (gov, mint) = (self.gov.insecure_clone(), self.mint.insecure_clone());
        const MINT_LEN: usize = 171; // base 165 + account type + DefaultAccountState TLV
        let rent = self.svm.minimum_balance_for_rent_exemption(MINT_LEN);
        let create = Instruction {
            program_id: SYSTEM,
            accounts: vec![
                AccountMeta::new(gov.pubkey(), true),
                AccountMeta::new(mint.pubkey(), true),
            ],
            data: {
                let mut d = 0u32.to_le_bytes().to_vec();
                d.extend_from_slice(&rent.to_le_bytes());
                d.extend_from_slice(&(MINT_LEN as u64).to_le_bytes());
                d.extend_from_slice(TOKEN_2022_ID.as_ref());
                d
            },
        };
        let das = Instruction {
            program_id: TOKEN_2022_ID,
            accounts: vec![AccountMeta::new(mint.pubkey(), false)],
            data: vec![28, 0, FROZEN],
        };
        let init_mint = Instruction {
            program_id: TOKEN_2022_ID,
            accounts: vec![AccountMeta::new(mint.pubkey(), false)],
            data: {
                let mut d = vec![20, 6];
                d.extend_from_slice(gov.pubkey().as_ref());
                d.push(1);
                d.extend_from_slice(gov.pubkey().as_ref());
                d
            },
        };
        let mut cfg = vec![0u8];
        cfg.extend_from_slice(PROGRAM_ID.as_ref());
        let acl = Instruction {
            program_id: TOKEN_ACL_ID,
            accounts: vec![
                AccountMeta::new(gov.pubkey(), true),
                AccountMeta::new_readonly(gov.pubkey(), true),
                AccountMeta::new(mint.pubkey(), false),
                AccountMeta::new(self.mint_config(), false),
                AccountMeta::new_readonly(SYSTEM, false),
                AccountMeta::new_readonly(TOKEN_2022_ID, false),
            ],
            data: cfg,
        };
        let toggle = Instruction {
            program_id: TOKEN_ACL_ID,
            accounts: vec![
                AccountMeta::new_readonly(gov.pubkey(), true),
                AccountMeta::new(self.mint_config(), false),
            ],
            data: vec![8, 1, 1],
        };
        self.ok(&[create, das, init_mint], &[&gov, &mint]);
        self.ok(&[acl, toggle], &[&gov]);
        // Pin the MintConfig layout token/scripts/verify.ts reads:
        // disc | bump | thaw_enabled | freeze_enabled | mint | freeze_authority | gating_program.
        let mc = self.svm.get_account(&self.mint_config()).unwrap();
        assert_eq!(mc.owner, TOKEN_ACL_ID);
        assert_eq!((mc.data[0], mc.data[2], mc.data[3]), (1, 1, 1));
        assert_eq!(&mc.data[4..36], mint.pubkey().as_ref());
        assert_eq!(&mc.data[68..100], PROGRAM_ID.as_ref());
        assert_eq!(mc.data.len(), 100);
    }

    /// A fresh Token-2022 account of the mint for `owner` (born frozen).
    fn token_account(&mut self, owner: &Pubkey) -> Pubkey {
        let (gov, ta) = (self.gov.insecure_clone(), Keypair::new());
        let rent = self.svm.minimum_balance_for_rent_exemption(165);
        let create = Instruction {
            program_id: SYSTEM,
            accounts: vec![
                AccountMeta::new(gov.pubkey(), true),
                AccountMeta::new(ta.pubkey(), true),
            ],
            data: {
                let mut d = 0u32.to_le_bytes().to_vec();
                d.extend_from_slice(&rent.to_le_bytes());
                d.extend_from_slice(&165u64.to_le_bytes());
                d.extend_from_slice(TOKEN_2022_ID.as_ref());
                d
            },
        };
        let init = Instruction {
            program_id: TOKEN_2022_ID,
            accounts: vec![
                AccountMeta::new(ta.pubkey(), false),
                AccountMeta::new_readonly(self.mint.pubkey(), false),
            ],
            data: {
                let mut d = vec![18];
                d.extend_from_slice(owner.as_ref());
                d
            },
        };
        self.ok(&[create, init], &[&gov, &ta]);
        assert_eq!(
            self.state(&ta.pubkey()),
            FROZEN,
            "DefaultAccountState = Frozen"
        );
        ta.pubkey()
    }

    fn state(&self, ta: &Pubkey) -> u8 {
        self.svm.get_account(ta).unwrap().data[STATE_OFFSET]
    }

    fn validate(&mut self, owner: Pubkey, investor: u8) -> ValidateDecision {
        let mint = self.mint.pubkey();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::Validate {
                config: self.config_pda(),
                policy: self.policy_pda(),
                block_entry: self.entry_pda(&owner),
                identity_attestation: self.attestation_address(&self.id_schema, &owner),
                accreditation_attestation: self.attestation_address(&self.acc_schema, &owner),
                holder_link: self.link_pda(&owner),
                investor: self.investor_pda(investor),
                lots: pda(
                    &[HolderLots::SEED, mint.as_ref(), owner.as_ref()],
                    &PROGRAM_ID,
                ),
                mint,
                payer: self.gov.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: ix::Validate { owner }.data(),
        };
        let gov = self.gov.insecure_clone();
        let logs = self.ok(&[ix], &[&gov]);
        let ev: Vec<ValidateDecision> = logs
            .iter()
            .filter_map(|l| l.strip_prefix("Program data: "))
            .filter_map(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .filter(|d| d.starts_with(ValidateDecision::DISCRIMINATOR))
            .map(|d| {
                ValidateDecision::try_from_slice(&d[ValidateDecision::DISCRIMINATOR.len()..])
                    .unwrap()
            })
            .collect();
        assert_eq!(ev.len(), 1, "every decision is evented (CG-P7)");
        ev.into_iter().next().unwrap()
    }

    /// The gate's extra accounts for `owner`, in the TLV's order. Token ACL
    /// resolves them from the TLV; the caller only has to supply them.
    fn extras(&self, owner: &Pubkey, metas_seed: &[u8]) -> Vec<AccountMeta> {
        let mint = self.mint.pubkey();
        let link = self.link_pda(owner);
        let (id, ac) = match self.svm.get_account(&link).filter(|a| !a.data.is_empty()) {
            Some(a) => {
                let l = HolderLink::try_deserialize(&mut a.data.as_slice()).unwrap();
                (l.identity_attestation, l.accreditation_attestation)
            }
            None => (
                self.attestation_address(&self.id_schema, owner),
                self.attestation_address(&self.acc_schema, owner),
            ),
        };
        vec![
            AccountMeta::new_readonly(pda(&[metas_seed, mint.as_ref()], &PROGRAM_ID), false),
            AccountMeta::new_readonly(self.config_pda(), false),
            AccountMeta::new_readonly(self.policy_pda(), false),
            AccountMeta::new_readonly(self.entry_pda(owner), false),
            AccountMeta::new_readonly(link, false),
            AccountMeta::new_readonly(id, false),
            AccountMeta::new_readonly(ac, false),
        ]
    }

    /// Token ACL `ThawPermissionless` (6) / `FreezePermissionless` (7),
    /// signed by an arbitrary third party: no gate authority involved.
    fn acl(&mut self, kind: u8, ta: Pubkey, owner: Pubkey) -> Result<(), u32> {
        let anyone = self.funded();
        let seed: &[u8] = if kind == 6 {
            b"thaw_extra_account_metas"
        } else {
            b"freeze_extra_account_metas"
        };
        let mut accounts = vec![
            AccountMeta::new(anyone.pubkey(), true),
            AccountMeta::new_readonly(self.mint.pubkey(), false),
            AccountMeta::new(ta, false),
            AccountMeta::new(pda(&[b"FLAG_ACCOUNT", ta.as_ref()], &TOKEN_ACL_ID), false),
            AccountMeta::new_readonly(owner, false),
            AccountMeta::new_readonly(self.mint_config(), false),
            AccountMeta::new_readonly(TOKEN_2022_ID, false),
            AccountMeta::new_readonly(SYSTEM, false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ];
        accounts.extend(self.extras(&owner, seed));
        let ix = Instruction {
            program_id: TOKEN_ACL_ID,
            accounts,
            data: vec![kind],
        };
        self.send(&[ix], &[&anyone]).map(|_| ()).map_err(|(c, _)| c)
    }

    fn thaw(&mut self, ta: Pubkey, owner: Pubkey) -> Result<(), u32> {
        self.acl(6, ta, owner)
    }
    fn freeze(&mut self, ta: Pubkey, owner: Pubkey) -> Result<(), u32> {
        self.acl(7, ta, owner)
    }

    fn policy(&self) -> MarketPolicy {
        let a = self.svm.get_account(&self.policy_pda()).unwrap();
        MarketPolicy::try_deserialize(&mut a.data.as_slice()).unwrap()
    }

    /// `release_holder` for `owner` (investor `id`), with the exit proof:
    /// its lot ledger, its Token-2022 ATA and `extra` token accounts.
    fn release_ix(&self, owner: Pubkey, id: u8, extra: &[Pubkey]) -> Instruction {
        let mint = self.mint.pubkey();
        let mut ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::ReleaseHolder {
                config: self.config_pda(),
                policy: self.policy_pda(),
                holder_link: self.link_pda(&owner),
                investor: self.investor_pda(id),
                mint,
                gate_config_authority: self.gov.pubkey(),
                lots: self.lots_pda(&owner),
                token_account: associated_token_2022(&owner, &mint),
            }
            .to_account_metas(None),
            data: ix::ReleaseHolder { owner }.data(),
        };
        ix.accounts
            .extend(extra.iter().map(|k| AccountMeta::new_readonly(*k, false)));
        ix
    }

    fn lots_pda(&self, owner: &Pubkey) -> Pubkey {
        pda(
            &[
                HolderLots::SEED,
                self.mint.pubkey().as_ref(),
                owner.as_ref(),
            ],
            &PROGRAM_ID,
        )
    }

    /// The gate authority records `qty` units for `owner` (`append_lot`).
    fn append_lot(&mut self, owner: Pubkey, qty: u64) {
        let mint = self.mint.pubkey();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AppendLot {
                config: self.config_pda(),
                lots: self.lots_pda(&owner),
                block_entry: self.entry_pda(&owner),
                mint,
                gate_config_authority: self.gov.pubkey(),
            }
            .to_account_metas(None),
            data: ix::AppendLot {
                qty,
                source: LotSource::PrimaryMint,
                affiliate: false,
                exemption_path: 0,
            }
            .data(),
        };
        let gov = self.gov.insecure_clone();
        self.ok(&[ix], &[&gov]);
    }

    /// `update_compliance` signed by `signer` (#202 tail).
    fn update_compliance(
        &mut self,
        signer: &Keypair,
        update: ComplianceUpdate,
    ) -> Result<Vec<String>, (u32, Vec<String>)> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::UpdateConfig {
                config: self.config_pda(),
                mint: self.mint.pubkey(),
                gate_config_authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix::UpdateCompliance { update }.data(),
        };
        self.send(&[ix], &[signer])
    }

    fn compliance(&mut self, update: ComplianceUpdate) {
        let gov = self.gov.insecure_clone();
        self.update_compliance(&gov, update)
            .unwrap_or_else(|(c, l)| panic!("update_compliance {c}: {l:#?}"));
    }

    fn set_seasoning(&mut self, secs: i64) {
        let gov = self.gov.insecure_clone();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::UpdateConfig {
                config: self.config_pda(),
                mint: self.mint.pubkey(),
                gate_config_authority: gov.pubkey(),
            }
            .to_account_metas(None),
            data: ix::UpdateConfig {
                policy_class: None,
                holder_cap: None,
                seasoning_seconds: Some(secs),
                settlement_horizon_seconds: None,
                attester_add: None,
                attester_remove: None,
                blocklist_set: None,
            }
            .data(),
        };
        self.ok(&[ix], &[&gov]);
    }

    fn block(&mut self, wallet: Pubkey) {
        let mint = self.mint.pubkey();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::BlockWallet {
                config: self.config_pda(),
                blocklist: BlockList::address(&mint),
                entry: self.entry_pda(&wallet),
                mint,
                gate_config_authority: self.gov.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: ix::BlockWallet {
                wallet,
                reason_hash: [1; 32],
            }
            .data(),
        };
        let gov = self.gov.insecure_clone();
        self.ok(&[ix], &[&gov]);
    }
}

/// Issues one wallet's attestations for a rejection case.
type Setup = Box<dyn Fn(&mut Env, Pubkey)>;

fn gate(e: GateError) -> u32 {
    u32::from(e)
}

// ── tests ─────────────────────────────────────────────────────────────

#[test]
fn an_attested_investor_thaws_through_token_acl_and_the_gate() {
    let mut env = Env::new(8);
    let alice = Pubkey::new_unique();
    let ta = env.token_account(&alice);
    // Not yet validated: Token ACL can't even resolve the gate's accounts.
    assert!(env.thaw(ta, alice).is_err());
    assert_eq!(env.state(&ta), FROZEN);

    let (id_att, acc_att) = env.attest_good(alice, 7);
    let d = env.validate(alice, 7);
    assert_eq!(d.reject, None);
    assert_eq!(d.distinct_investors, 1);
    assert_ne!(d.attestation_inputs_hash, [0; 32]);
    // #131: the decision names what it rests on: each attestation, its
    // attester, its schema and its expiry, as SAS holds them.
    let att = env.attester.pubkey();
    assert_eq!(
        (
            d.identity.attestation,
            d.identity.attester,
            d.identity.schema,
            d.identity.expiry
        ),
        (id_att, att, env.id_schema, NOW + 90 * DAY)
    );
    assert_eq!(
        (
            d.accreditation.attestation,
            d.accreditation.attester,
            d.accreditation.schema,
            d.accreditation.expiry
        ),
        (acc_att, att, env.acc_schema, NOW + 90 * DAY)
    );
    // validate also opened the lot ledger (CG-P1 step 5).
    let lots = pda(
        &[HolderLots::SEED, env.mint.pubkey().as_ref(), alice.as_ref()],
        &PROGRAM_ID,
    );
    assert!(env
        .svm
        .get_account(&lots)
        .is_some_and(|a| !a.data.is_empty()));

    env.thaw(ta, alice).unwrap();
    assert_eq!(
        env.state(&ta),
        INITIALIZED,
        "thawed by Token ACL after the gate approved"
    );
    // An eligible holder can't be frozen by just anyone.
    assert_eq!(env.freeze(ta, alice), Err(gate(GateError::StillEligible)));
}

#[test]
fn every_rejection_is_typed_and_changes_nothing() {
    let mut env = Env::new(8);
    let (id, ac) = (env.id_schema, env.acc_schema);
    let horizon_short = NOW + DAY - 1; // expires inside the 1-day settlement horizon
    let cases: Vec<(&str, Setup, GateReject)> = vec![
        (
            "not accredited",
            Box::new(move |e: &mut Env, w| {
                e.attest(id, w, identity_data(1, b"US", 2), 0);
                e.attest(ac, w, vec![0, 1], 0);
            }),
            GateReject::NotAccredited,
        ),
        (
            "jurisdiction",
            Box::new(move |e: &mut Env, w| {
                e.attest(id, w, identity_data(2, b"IN", 2), 0);
                e.attest(ac, w, vec![1, 1], 0);
            }),
            GateReject::JurisdictionRejected,
        ),
        (
            "kyc level",
            Box::new(move |e: &mut Env, w| {
                e.attest(id, w, identity_data(3, b"US", 0), 0);
                e.attest(ac, w, vec![1, 1], 0);
            }),
            GateReject::KycLevelTooLow,
        ),
        (
            "expiry inside the settlement horizon",
            Box::new(move |e: &mut Env, w| {
                e.attest(id, w, identity_data(4, b"US", 2), horizon_short);
                e.attest(ac, w, vec![1, 1], 0);
            }),
            GateReject::AttestationExpiry,
        ),
        (
            "no accreditation attestation at all",
            Box::new(move |e: &mut Env, w| {
                e.attest(id, w, identity_data(5, b"US", 2), 0);
            }),
            GateReject::AttestationInvalid,
        ),
        (
            "malformed identity data",
            Box::new(move |e: &mut Env, w| {
                // SAS accepts it (layout-valid, lower-case code); the gate doesn't.
                e.attest(id, w, identity_data(6, b"us", 2), 0);
                e.attest(ac, w, vec![1, 1], 0);
            }),
            GateReject::AttestationInvalid,
        ),
    ];
    for (name, setup, want) in cases {
        let w = Pubkey::new_unique();
        setup(&mut env, w);
        let d = env.validate(w, 0);
        assert_eq!(d.reject, Some(want), "{name}");
        assert!(
            env.svm.get_account(&env.link_pda(&w)).is_none(),
            "{name}: no link written"
        );
        assert_eq!(
            env.policy().distinct_investors,
            0,
            "{name}: governor untouched"
        );
    }
}

#[test]
fn an_attester_outside_the_market_set_is_not_trusted() {
    let mut env = Env::new(8);
    // A second signer the credential authorizes, but the market does not.
    let rogue = env.funded();
    let mut data = vec![3u8]; // ChangeAuthorizedSigners
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(env.attester.pubkey().as_ref());
    data.extend_from_slice(rogue.pubkey().as_ref());
    let ix = Instruction {
        program_id: SAS_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(env.cred_auth.pubkey(), true),
            AccountMeta::new_readonly(env.cred_auth.pubkey(), true),
            AccountMeta::new(env.credential, false),
            AccountMeta::new_readonly(SYSTEM, false),
        ],
        data,
    };
    let ca = env.cred_auth.insecure_clone();
    env.ok(&[ix], &[&ca]);
    let w = Pubkey::new_unique();
    let real = std::mem::replace(&mut env.attester, rogue);
    env.attest_good(w, 9);
    env.attester = real;
    assert_eq!(
        env.validate(w, 9).reject,
        Some(GateReject::AttestationInvalid)
    );
}

#[test]
fn the_block_list_wins_over_valid_attestations() {
    let mut env = Env::new(8);
    let bob = Pubkey::new_unique();
    let ta = env.token_account(&bob);
    env.attest_good(bob, 3);
    assert_eq!(env.validate(bob, 3).reject, None);
    env.block(bob);
    assert_eq!(env.thaw(ta, bob), Err(gate(GateError::Blocklisted)));
    assert_eq!(env.state(&ta), FROZEN);
    let w = Pubkey::new_unique();
    env.attest_good(w, 4);
    env.block(w);
    let d = env.validate(w, 4);
    assert_eq!(d.reject, Some(GateReject::Blocklisted));
    assert_eq!(
        (d.identity, d.accreditation),
        Default::default(),
        "a rejection names no attestation"
    );
}

#[test]
fn revocation_in_sas_lets_anyone_freeze() {
    let mut env = Env::new(8);
    let carol = Pubkey::new_unique();
    let ta = env.token_account(&carol);
    let (id_att, _) = env.attest_good(carol, 5);
    assert_eq!(env.validate(carol, 5).reject, None);
    env.thaw(ta, carol).unwrap();
    assert_eq!(env.state(&ta), INITIALIZED);
    // The attester revokes: SAS closes the identity attestation.
    env.sas_close(id_att);
    env.freeze(ta, carol).unwrap();
    assert_eq!(
        env.state(&ta),
        FROZEN,
        "frozen by a third party once ineligible"
    );
    assert_eq!(
        env.thaw(ta, carol),
        Err(gate(GateError::AttestationInvalid))
    );
}

#[test]
fn expiry_lapses_into_the_horizon_and_blocks_the_thaw() {
    let mut env = Env::new(8);
    let dan = Pubkey::new_unique();
    let ta = env.token_account(&dan);
    let (id, ac) = (env.id_schema, env.acc_schema);
    env.attest(id, dan, identity_data(6, b"US", 2), NOW + 10 * DAY);
    env.attest(ac, dan, vec![1, 1], 0);
    assert_eq!(env.validate(dan, 6).reject, None);
    // Nine and a half days later the attestation no longer spans the horizon.
    let mut c: Clock = env.svm.get_sysvar();
    c.unix_timestamp = NOW + 10 * DAY - DAY / 2;
    env.svm.set_sysvar(&c);
    assert_eq!(env.thaw(ta, dan), Err(gate(GateError::AttestationExpired)));
}

#[test]
fn the_governor_counts_investors_not_wallets() {
    let mut env = Env::new(2);
    let (a1, a2, b, c) = (
        Pubkey::new_unique(),
        Pubkey::new_unique(),
        Pubkey::new_unique(),
        Pubkey::new_unique(),
    );
    env.attest_good(a1, 1);
    env.attest_good(a2, 1); // investor 1's second wallet
    env.attest_good(b, 2);
    env.attest_good(c, 3);
    assert_eq!(env.validate(a1, 1).reject, None);
    assert_eq!(env.validate(a2, 1).reject, None);
    assert_eq!(
        env.policy().distinct_investors,
        1,
        "two wallets, one investor"
    );
    assert_eq!(env.validate(b, 2).reject, None);
    assert_eq!(env.validate(c, 3).reject, Some(GateReject::GovernorCap));
    assert_eq!(env.policy().distinct_investors, 2);

    // project_and_check: known investors pass, a new one does not.
    let check = |env: &mut Env, id: u8| {
        let mint = env.mint.pubkey();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::ProjectAndCheck {
                config: env.config_pda(),
                policy: env.policy_pda(),
                investor: env.investor_pda(id),
                mint,
            }
            .to_account_metas(None),
            data: ix::ProjectAndCheck {
                _investor_id: [id; 16],
            }
            .data(),
        };
        let p = env.funded();
        env.send(&[ix], &[&p]).map(|_| ()).map_err(|(c, _)| c)
    };
    assert_eq!(check(&mut env, 1), Ok(()));
    assert_eq!(check(&mut env, 3), Err(gate(GateError::GovernorCap)));

    // Investor 2 exits to zero: its slot frees up for investor 3.
    let release = |env: &mut Env, owner: Pubkey, id: u8| {
        let ix = env.release_ix(owner, id, &[]);
        let gov = env.gov.insecure_clone();
        env.ok(&[ix], &[&gov]);
    };
    release(&mut env, a1, 1);
    assert_eq!(
        env.policy().distinct_investors,
        2,
        "investor 1 still has a wallet"
    );
    release(&mut env, b, 2);
    assert_eq!(env.policy().distinct_investors, 1);
    assert_eq!(env.validate(c, 3).reject, None);
}

#[test]
fn a_wallet_cannot_be_re_mapped_to_another_investor() {
    let mut env = Env::new(8);
    let e = Pubkey::new_unique();
    env.attest_good(e, 1);
    assert_eq!(env.validate(e, 1).reject, None);
    // Attester re-issues the identity under a different investor_id.
    let (id, _) = (env.id_schema, env.acc_schema);
    let old = env.attestation_address(&id, &e);
    env.sas_close(old);
    env.attest(id, e, identity_data(2, b"US", 2), 0);
    assert_eq!(
        env.validate(e, 2).reject,
        Some(GateReject::InvestorIdChanged)
    );
}

#[test]
fn the_hook_refuses_calls_from_outside_token_acl() {
    let mut env = Env::new(8);
    let f = Pubkey::new_unique();
    let ta = env.token_account(&f);
    env.attest_good(f, 1);
    assert_eq!(env.validate(f, 1).reject, None);
    // Call the gate's hook directly: no Token ACL flag account set.
    let mut accounts = vec![
        AccountMeta::new_readonly(env.gov.pubkey(), true),
        AccountMeta::new_readonly(ta, false),
        AccountMeta::new_readonly(env.mint.pubkey(), false),
        AccountMeta::new_readonly(f, false),
        AccountMeta::new_readonly(pda(&[b"FLAG_ACCOUNT", ta.as_ref()], &TOKEN_ACL_ID), false),
    ];
    accounts.extend(env.extras(&f, b"thaw_extra_account_metas"));
    let ix = Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data: credit_gate::CAN_THAW_DISCRIMINATOR.to_vec(),
    };
    let gov = env.gov.insecure_clone();
    assert_eq!(
        env.send(&[ix], &[&gov]).map(|_| ()).map_err(|(c, _)| c),
        Err(gate(GateError::NotTokenAclFlow))
    );
}

#[test]
fn policy_refuses_schemas_outside_the_credential() {
    // Market whose config names schema accounts that are not SAS schemas.
    let mut env = Env::new(8);
    let other_mint = Pubkey::new_unique();
    let gov = env.gov.insecure_clone();
    let cfg = pda(&[MarketConfig::SEED, other_mint.as_ref()], &PROGRAM_ID);
    let bogus = Pubkey::new_unique();
    let init = Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::InitializeConfig {
            config: cfg,
            root: root_pda(),
            mint: other_mint,
            authority: gov.pubkey(),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: ix::InitializeConfig {
            policy_class: 1,
            holder_cap: 8,
            seasoning_seconds: 7 * DAY,
            settlement_horizon_seconds: DAY,
            attesters: vec![env.attester.pubkey()],
            identity_schema: bogus,
            accreditation_schema: env.acc_schema,
        }
        .data(),
    };
    env.ok(&[init], &[&gov]);
    let policy = Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::InitializePolicy {
            config: cfg,
            policy: pda(&[MarketPolicy::SEED, other_mint.as_ref()], &PROGRAM_ID),
            identity_schema: bogus,
            accreditation_schema: env.acc_schema,
            mint: other_mint,
            gate_config_authority: gov.pubkey(),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: ix::InitializePolicy {
            credential: env.credential,
            jurisdictions: vec![*b"US"],
            min_kyc_level: 1,
            require_accreditation: true,
        }
        .data(),
    };
    assert_eq!(
        env.send(&[policy], &[&gov]).map(|_| ()).map_err(|(c, _)| c),
        Err(gate(GateError::InvalidSchema))
    );
}

fn events<T: AnchorDeserialize + Discriminator>(logs: &[String]) -> Vec<T> {
    logs.iter()
        .filter_map(|l| l.strip_prefix("Program data: "))
        .filter_map(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .filter(|d| d.starts_with(T::DISCRIMINATOR))
        .map(|d| T::try_from_slice(&d[T::DISCRIMINATOR.len()..]).unwrap())
        .collect()
}

/// CG-P4 (#202): the governor's count drops only on proof of the exit to
/// zero: a holder whose ledger still holds units, or who is shown with a
/// token account that is not its own, is not released.
#[test]
fn release_needs_proof_the_holder_exited_to_zero() {
    let mut env = Env::new(8);
    let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
    env.attest_good(a, 1);
    env.attest_good(b, 2);
    assert_eq!(env.validate(a, 1).reject, None);
    assert_eq!(env.validate(b, 2).reject, None);
    env.append_lot(a, 5);
    let gov = env.gov.insecure_clone();
    let ix = env.release_ix(a, 1, &[]);
    assert_eq!(
        env.send(&[ix], &[&gov]).unwrap_err().0,
        gate(GateError::HolderNotExited),
        "units still on the ledger"
    );
    // Someone else's token account is not proof of anything.
    let other = env.token_account(&Pubkey::new_unique());
    let ix = env.release_ix(b, 2, &[other]);
    assert_eq!(
        env.send(&[ix], &[&gov]).unwrap_err().0,
        gate(GateError::HolderNotExited)
    );
    // Its own empty account is.
    let own = env.token_account(&b);
    let ix = env.release_ix(b, 2, &[own]);
    env.ok(&[ix], &[&gov]);
    assert_eq!(env.policy().distinct_investors, 1);
}

/// #202: the compliance tail lives behind the unchanged `MarketConfig`. A
/// new market carries it; a market created before #202 reads the defaults
/// (its old behaviour) until anyone runs `migrate_config`, and the config
/// decodes as a `MarketConfig` throughout.
#[test]
fn the_compliance_tail_migrates_in_place_behind_an_unchanged_config() {
    let mut env = Env::new(8);
    let cfg = env.config_pda();
    let len = env.svm.get_account(&cfg).unwrap().data.len();
    assert_eq!(len, Compliance::CONFIG_SPACE, "new markets have the tail");
    // Cut the account back to the pre-#202 size.
    let mut acct = env.svm.get_account(&cfg).unwrap();
    acct.data.truncate(Compliance::OFFSET);
    env.svm.set_account(cfg, acct).unwrap();
    let before =
        MarketConfig::try_deserialize(&mut env.svm.get_account(&cfg).unwrap().data.as_slice())
            .unwrap();
    let gov = env.gov.insecure_clone();
    assert_eq!(
        env.update_compliance(
            &gov,
            ComplianceUpdate {
                min_ticket: Some(1),
                ..Default::default()
            }
        )
        .unwrap_err()
        .0,
        gate(GateError::ConfigNotMigrated)
    );
    // An old config still admits, on the defaults.
    let w = Pubkey::new_unique();
    env.attest_good(w, 1);
    assert_eq!(env.validate(w, 1).reject, None);

    let payer = env.funded();
    let migrate = |env: &mut Env, payer: &Keypair| {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::MigrateConfig {
                config: env.config_pda(),
                mint: env.mint.pubkey(),
                payer: payer.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: ix::MigrateConfig {}.data(),
        };
        env.send(&[ix], &[payer])
    };
    let logs = migrate(&mut env, &payer).expect("migrate");
    let ev = &events::<ConfigUpdated>(&logs)[0];
    assert_eq!(ev.field_group_ext, field_group_ext::MIGRATED);
    let acct = env.svm.get_account(&cfg).unwrap();
    assert_eq!(acct.data.len(), Compliance::CONFIG_SPACE);
    let after = MarketConfig::try_deserialize(&mut acct.data.as_slice()).unwrap();
    assert_eq!(
        after.fingerprint(),
        before.fingerprint(),
        "config untouched"
    );
    assert_eq!(
        migrate(&mut env, &payer).unwrap_err().0,
        gate(GateError::ConfigAlreadyMigrated)
    );
    env.compliance(ComplianceUpdate {
        min_ticket: Some(1),
        ..Default::default()
    });
}

/// CG-3 / CG-4 / CG-6 (#202): `update_compliance` is the gate authority's,
/// writes something, and keeps the tail in bounds: a Reg S market needs a
/// distribution-compliance period of at least 40 days and no longer than
/// its seasoning; an affiliate limit needs a window of at least a day.
#[test]
fn compliance_updates_are_governed_and_bounded() {
    let mut env = Env::new(8);
    let stranger = env.funded();
    assert_eq!(
        env.update_compliance(
            &stranger,
            ComplianceUpdate {
                min_ticket: Some(1),
                ..Default::default()
            }
        )
        .unwrap_err()
        .0,
        gate(GateError::Unauthorized)
    );
    let gov = env.gov.insecure_clone();
    assert_eq!(
        env.update_compliance(&gov, ComplianceUpdate::default())
            .unwrap_err()
            .0,
        gate(GateError::EmptyComplianceUpdate)
    );
    let reg_s = |secs: i64| ComplianceUpdate {
        exemption_class: Some(ExemptionClass::RegSNonUs),
        distribution_compliance_seconds: Some(secs),
        ..Default::default()
    };
    // Seasoning is 7 days: a 40-day period does not fit under it.
    assert_eq!(
        env.update_compliance(&gov, reg_s(40 * DAY)).unwrap_err().0,
        gate(GateError::ConfigOutOfBounds)
    );
    env.set_seasoning(40 * DAY);
    assert_eq!(
        env.update_compliance(&gov, reg_s(39 * DAY)).unwrap_err().0,
        gate(GateError::ConfigOutOfBounds),
        "under Reg S's shortest period"
    );
    env.compliance(reg_s(40 * DAY));
    // Seasoning cannot now drop under the period.
    let ix = Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::UpdateConfig {
            config: env.config_pda(),
            mint: env.mint.pubkey(),
            gate_config_authority: gov.pubkey(),
        }
        .to_account_metas(None),
        data: ix::UpdateConfig {
            policy_class: None,
            holder_cap: None,
            seasoning_seconds: Some(7 * DAY),
            settlement_horizon_seconds: None,
            attester_add: None,
            attester_remove: None,
            blocklist_set: None,
        }
        .data(),
    };
    assert_eq!(
        env.send(&[ix], &[&gov]).unwrap_err().0,
        gate(GateError::ConfigOutOfBounds)
    );
    assert_eq!(
        env.update_compliance(
            &gov,
            ComplianceUpdate {
                affiliate_volume_limit: Some(100),
                affiliate_window_seconds: Some(60),
                ..Default::default()
            }
        )
        .unwrap_err()
        .0,
        gate(GateError::ConfigOutOfBounds)
    );
}

/// CG-3 (#202): the exemption class narrows the policy at admission: a
/// Reg S market admits no `US` identity, an `EU_MTF_only` market admits
/// nobody (routed out in v1–v2). Both rejections are typed and evented.
#[test]
fn the_exemption_class_narrows_admission() {
    let mut env = Env::new(8);
    env.set_seasoning(40 * DAY);
    env.compliance(ComplianceUpdate {
        exemption_class: Some(ExemptionClass::RegSNonUs),
        distribution_compliance_seconds: Some(40 * DAY),
        ..Default::default()
    });
    let us = Pubkey::new_unique();
    env.attest_good(us, 1);
    assert_eq!(
        env.validate(us, 1).reject,
        Some(GateReject::JurisdictionRejected),
        "Reg S: no US holder"
    );
    env.compliance(ComplianceUpdate {
        exemption_class: Some(ExemptionClass::EuMtfOnly),
        distribution_compliance_seconds: Some(0),
        ..Default::default()
    });
    assert_eq!(
        env.validate(us, 1).reject,
        Some(GateReject::ExemptionClassClosed)
    );
    env.compliance(ComplianceUpdate {
        exemption_class: Some(ExemptionClass::PolicyOnly),
        ..Default::default()
    });
    assert_eq!(env.validate(us, 1).reject, None, "back to the policy alone");
}

/// D14 / doc 09 Path 7 and L6 (#202): a permanent-delegate seizure is
/// recorded by the gate authority with a reason hash; affiliates are
/// registered and deregistered per market. Both are the authority's only.
#[test]
fn seizures_are_recorded_and_affiliates_registered_by_the_authority() {
    let mut env = Env::new(8);
    let mint = env.mint.pubkey();
    let gov = env.gov.insecure_clone();
    let (from, to) = (Pubkey::new_unique(), Pubkey::new_unique());
    let seize = |env: &Env, signer: &Pubkey, amount: u64| Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::RecordSeizure {
            config: env.config_pda(),
            mint,
            gate_config_authority: *signer,
        }
        .to_account_metas(None),
        data: ix::RecordSeizure {
            from,
            to,
            amount,
            reason_hash: [0x5e; 32],
        }
        .data(),
    };
    let stranger = env.funded();
    assert_eq!(
        env.send(&[seize(&env, &stranger.pubkey(), 7)], &[&stranger])
            .unwrap_err()
            .0,
        gate(GateError::Unauthorized)
    );
    assert_eq!(
        env.send(&[seize(&env, &gov.pubkey(), 0)], &[&gov])
            .unwrap_err()
            .0,
        gate(GateError::EmptySeizure)
    );
    let logs = env.ok(&[seize(&env, &gov.pubkey(), 7)], &[&gov]);
    let ev = &events::<SeizureRecorded>(&logs)[0];
    assert_eq!((ev.from, ev.to, ev.amount), (from, to, 7));
    assert_eq!(ev.reason_hash, [0x5e; 32]);

    let wallet = Pubkey::new_unique();
    let record = AffiliateRecord::address(&mint, &wallet);
    let reg = Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::RegisterAffiliate {
            config: env.config_pda(),
            record,
            mint,
            gate_config_authority: gov.pubkey(),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: ix::RegisterAffiliate { wallet }.data(),
    };
    let logs = env.ok(&[reg], &[&gov]);
    assert!(events::<AffiliateSet>(&logs)[0].registered);
    let a = env.svm.get_account(&record).unwrap();
    let r = AffiliateRecord::try_deserialize(&mut a.data.as_slice()).unwrap();
    assert_eq!((r.wallet, r.window_qty), (wallet, 0));
    let dereg = Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::DeregisterAffiliate {
            config: env.config_pda(),
            record,
            mint,
            gate_config_authority: gov.pubkey(),
        }
        .to_account_metas(None),
        data: ix::DeregisterAffiliate {}.data(),
    };
    let logs = env.ok(&[dereg], &[&gov]);
    assert!(!events::<AffiliateSet>(&logs)[0].registered);
    assert!(env
        .svm
        .get_account(&record)
        .is_none_or(|a| a.data.is_empty()));
}
