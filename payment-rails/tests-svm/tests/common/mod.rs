//! Shared LiteSVM harness: loads the real `anchor build` artifact as an
//! upgradeable program, fabricates a 6-dp settlement mint under either token
//! program plus a Token-2022 asset mint (like NPCS), and wraps every rails
//! instruction.

#![allow(dead_code)]

mod engines;
mod gate;
#[allow(unused_imports)]
pub use engines::*;
pub use gate::*;

use anchor_lang::prelude::{Clock, Pubkey};
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::token_2022::spl_token_2022;
use litesvm::types::TransactionMetadata;
use litesvm::LiteSVM;
use payment_rails::{
    accounts as acc, instruction as ix, Advance, Breaker, CashSession, CouponParams, CouponReceipt,
    Escrow, Facility, FillReceipt, PayoutDestination, RailsConfig, RailsError, SessionParams,
};
use solana_account::Account;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_program_pack::Pack;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

pub const PROGRAM_ID: Pubkey = payment_rails::ID;
pub const T0: i64 = 1_000_000;
pub const COMMIT_END: i64 = T0 + 100;
pub const REVEAL_END: i64 = T0 + 200;
pub const SETTLE_DEADLINE: i64 = T0 + 300;
pub const BOND: u64 = 5_000_000; // 5 USDC
pub const MAX_CASH: u64 = 1_000_000_000; // 1,000 USDC
pub use credit_math::MAX_ADVANCE_RATE_BPS;

pub(crate) const LOADER_V3: Pubkey = anchor_lang::solana_program::bpf_loader_upgradeable::ID;

/// Which token program owns a mint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tp {
    Spl,
    T22,
}

impl Tp {
    pub fn id(self) -> Pubkey {
        match self {
            Tp::Spl => anchor_spl::token::ID,
            Tp::T22 => anchor_spl::token_2022::ID,
        }
    }
    pub const ALL: [Tp; 2] = [Tp::Spl, Tp::T22];
}

/// Why a transaction failed, reduced to what tests assert on.
#[derive(Debug, PartialEq, Eq)]
pub enum Fail {
    /// Custom program error code (anchor: 6000+ ours, 2000–4999 framework).
    Code(u32),
    /// Any other failure (account in use, missing account, …).
    Other(String),
}

impl Fail {
    pub fn rails(e: RailsError) -> Fail {
        Fail::Code(u32::from(e))
    }
}

pub type Res = Result<TransactionMetadata, Fail>;

/// A participant: wallet + USDC account + asset account.
pub struct Party {
    pub key: Keypair,
    pub usdc: Pubkey,
    pub asset: Pubkey,
}

/// Account set for one `settle_fill`, defaulted from the parties; tests
/// override fields to attempt spoofing.
#[derive(Clone, Copy)]
pub struct Fill {
    pub session: Pubkey,
    pub buyer_owner: Pubkey,
    pub seller_owner: Pubkey,
    pub seller_usdc: Pubkey,
    pub buyer_asset: Pubkey,
    pub fee_receiver: Pubkey,
    pub reserve: Pubkey,
}

pub struct Env {
    pub svm: LiteSVM,
    /// Token program of the settlement mint (varies per test run).
    pub tp: Tp,
    /// Token program of the traded asset: always Token-2022, like NPCS.
    pub asset_tp: Tp,
    pub authority: Keypair,
    pub mint: Pubkey,
    pub asset_mint: Pubkey,
    pub fee: Pubkey,
    pub reserve: Pubkey,
    pub config: Pubkey,
    /// SAS credential, schemas and attester of the gate market (#133).
    pub sas: Sas,
}

pub fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &PROGRAM_ID).0
}

pub fn session_pda(operator: &Pubkey, id: u64) -> Pubkey {
    pda(&[CashSession::SEED, operator.as_ref(), &id.to_le_bytes()])
}

pub fn escrow_pda(session: &Pubkey, owner: &Pubkey) -> Pubkey {
    pda(&[Escrow::SEED, session.as_ref(), owner.as_ref()])
}

pub fn vault_pda(escrow: &Pubkey) -> Pubkey {
    pda(&[Escrow::VAULT_SEED, escrow.as_ref()])
}

pub fn asset_vault_pda(escrow: &Pubkey) -> Pubkey {
    pda(&[Escrow::ASSET_VAULT_SEED, escrow.as_ref()])
}

pub fn dest_pda(owner: &Pubkey) -> Pubkey {
    pda(&[PayoutDestination::SEED, owner.as_ref()])
}

pub fn facility_pda(curator: &Pubkey, id: u64) -> Pubkey {
    pda(&[Facility::SEED, curator.as_ref(), &id.to_le_bytes()])
}

pub fn facility_vault_pda(facility: &Pubkey) -> Pubkey {
    pda(&[Facility::VAULT_SEED, facility.as_ref()])
}

pub fn advance_pda(facility: &Pubkey, borrower: &Pubkey) -> Pubkey {
    pda(&[Advance::SEED, facility.as_ref(), borrower.as_ref()])
}

pub fn coupon_pda(distributor: &Pubkey, period: u64) -> Pubkey {
    pda(&[
        CouponReceipt::SEED,
        distributor.as_ref(),
        &period.to_le_bytes(),
    ])
}

pub fn receipt_pda(session: &Pubkey, fill_index: u32) -> Pubkey {
    pda(&[
        FillReceipt::SEED,
        session.as_ref(),
        &fill_index.to_le_bytes(),
    ])
}

impl Env {
    /// Fresh VM + deployed program + initialized config at `T0`.
    pub fn new(tp: Tp) -> Env {
        let so = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/deploy/payment_rails.so"
        );
        let bytes = std::fs::read(so)
            .unwrap_or_else(|e| panic!("{so}: {e} — run `anchor build` in payment-rails/ first"));
        let mut svm = LiteSVM::new();
        svm.add_program(PROGRAM_ID, &bytes).expect("load program");

        // LiteSVM deploys with no upgrade authority; install one as a real
        // deploy would. ProgramData header: u32 tag | u64 slot | Option<Pubkey>.
        let authority = Keypair::new();
        let (programdata, _) = Pubkey::find_program_address(&[PROGRAM_ID.as_ref()], &LOADER_V3);
        let mut pd = svm.get_account(&programdata).expect("programdata");
        pd.data[12] = 1;
        pd.data[13..45].copy_from_slice(authority.pubkey().as_ref());
        svm.set_account(programdata, pd).unwrap();

        svm.airdrop(&authority.pubkey(), 100_000_000_000).unwrap();
        let mut env = Env {
            svm,
            tp,
            asset_tp: Tp::T22,
            authority,
            mint: Pubkey::default(),
            asset_mint: Pubkey::default(),
            fee: Pubkey::default(),
            reserve: Pubkey::default(),
            config: pda(&[RailsConfig::SEED]),
            sas: Sas {
                cred_auth: Keypair::new(),
                attester: Keypair::new(),
                credential: Pubkey::default(),
                id_schema: Pubkey::default(),
                acc_schema: Pubkey::default(),
                nav_schema: Pubkey::default(),
                rr_schema: Pubkey::default(),
            },
        };
        env.set_time(T0);
        env.sas = env.load_gate();
        env.mint = env.new_mint(6);
        env.asset_mint = env.new_mint_in(Tp::T22, 6);
        env.open_gate_market();
        let auth = env.authority.pubkey();
        let mint = env.mint;
        env.fee = env.token_account(&mint, &auth, 0);
        env.reserve = env.token_account(&mint, &auth, 0);

        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeConfig {
                config: env.config,
                usdc_mint: env.mint,
                fee_receiver: env.fee,
                reserve: env.reserve,
                authority: auth,
                program: PROGRAM_ID,
                program_data: programdata,
                token_program: tp.id(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::InitializeConfig {}.data(),
        };
        let a = env.authority.insecure_clone();
        env.send(&[ix], &[&a]).expect("initialize_config");
        env
    }

    // ── VM plumbing ─────────────────────────────────────────────────────

    pub fn set_time(&mut self, unix_timestamp: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp = unix_timestamp;
        self.svm.set_sysvar(&clock);
    }

    pub fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Res {
        let payer = signers[0].pubkey();
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&payer),
            signers,
            self.svm.latest_blockhash(),
        );
        // New blockhash per tx so an identical retry is a new transaction.
        let out = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        out.map_err(|f| match f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(c)) => Fail::Code(c),
            other => Fail::Other(format!("{other:?} | {:?}", f.meta.logs)),
        })
    }

    pub fn funded(&mut self) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        k
    }

    pub fn lamports(&self, who: &Pubkey) -> u64 {
        self.svm.get_account(who).map(|a| a.lamports).unwrap_or(0)
    }

    pub fn exists(&self, who: &Pubkey) -> bool {
        self.svm
            .get_account(who)
            .is_some_and(|a| a.lamports > 0 || !a.data.is_empty())
    }

    // ── token fabrication (state written directly; programs are real) ───

    pub fn new_mint(&mut self, decimals: u8) -> Pubkey {
        self.new_mint_in(self.tp, decimals)
    }

    pub fn new_mint_in(&mut self, program: Tp, decimals: u8) -> Pubkey {
        let key = Pubkey::new_unique();
        let mut data = vec![0u8; spl_token_2022::state::Mint::LEN];
        spl_token_2022::state::Mint {
            mint_authority: Some(self.authority.pubkey()).into(),
            supply: u64::MAX / 2,
            decimals,
            is_initialized: true,
            freeze_authority: None.into(),
        }
        .pack_into_slice(&mut data);
        self.put(key, data, program);
        key
    }

    /// Settlement-mint (USDC) account.
    pub fn token_account(&mut self, mint: &Pubkey, owner: &Pubkey, amount: u64) -> Pubkey {
        self.token_account_in(self.tp, mint, owner, amount)
    }

    /// Asset (Token-2022) account.
    pub fn asset_account(&mut self, owner: &Pubkey, amount: u64) -> Pubkey {
        let mint = self.asset_mint;
        self.token_account_in(self.asset_tp, &mint, owner, amount)
    }

    pub fn token_account_in(
        &mut self,
        program: Tp,
        mint: &Pubkey,
        owner: &Pubkey,
        amount: u64,
    ) -> Pubkey {
        let key = Pubkey::new_unique();
        let mut data = vec![0u8; spl_token_2022::state::Account::LEN];
        spl_token_2022::state::Account {
            mint: *mint,
            owner: *owner,
            amount,
            delegate: None.into(),
            state: spl_token_2022::state::AccountState::Initialized,
            is_native: None.into(),
            delegated_amount: 0,
            close_authority: None.into(),
        }
        .pack_into_slice(&mut data);
        self.put(key, data, program);
        key
    }

    /// Credit `amount` more asset to an existing (fabricated) account.
    pub fn raw_asset_mint_to(&mut self, account: &Pubkey, amount: u64) {
        let mut a = self.svm.get_account(account).expect("asset account");
        let mut t = spl_token_2022::state::Account::unpack_from_slice(&a.data).unwrap();
        t.amount += amount;
        t.pack_into_slice(&mut a.data);
        self.svm.set_account(*account, a).unwrap();
    }

    fn put(&mut self, key: Pubkey, data: Vec<u8>, program: Tp) {
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                key,
                Account {
                    lamports,
                    data,
                    owner: program.id(),
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    /// The owner of a token account.
    pub fn token_owner(&self, token_account: &Pubkey) -> Pubkey {
        let a = self.svm.get_account(token_account).expect("token account");
        spl_token_2022::state::Account::unpack_from_slice(&a.data[..165])
            .unwrap()
            .owner
    }

    /// Token balance; 0 for a closed/missing account.
    pub fn bal(&self, token_account: &Pubkey) -> u64 {
        match self.svm.get_account(token_account) {
            Some(a) if !a.data.is_empty() => {
                spl_token_2022::state::Account::unpack_from_slice(&a.data[..165])
                    .unwrap()
                    .amount
            }
            _ => 0,
        }
    }

    /// Plain settlement-mint transfer by `owner` (used to grief vaults).
    pub fn raw_transfer(
        &mut self,
        from: &Pubkey,
        to: &Pubkey,
        owner: &Keypair,
        amount: u64,
    ) -> Res {
        let ix = spl_token_2022::instruction::transfer_checked(
            &self.tp.id(),
            from,
            &self.mint,
            to,
            &owner.pubkey(),
            &[],
            amount,
            6,
        )
        .unwrap();
        self.send(&[ix], &[owner])
    }

    /// Plain asset transfer by `owner` (used to grief asset vaults).
    pub fn raw_asset_transfer(
        &mut self,
        from: &Pubkey,
        to: &Pubkey,
        owner: &Keypair,
        amount: u64,
    ) -> Res {
        let ix = spl_token_2022::instruction::transfer_checked(
            &self.asset_tp.id(),
            from,
            &self.asset_mint,
            to,
            &owner.pubkey(),
            &[],
            amount,
            6,
        )
        .unwrap();
        self.send(&[ix], &[owner])
    }

    // ── account readers ─────────────────────────────────────────────────

    pub fn read<T: AccountDeserialize>(&self, key: &Pubkey) -> T {
        let a = self.svm.get_account(key).expect("account exists");
        T::try_deserialize(&mut a.data.as_slice()).expect("deserialize")
    }

    pub fn session(&self, key: &Pubkey) -> CashSession {
        self.read(key)
    }

    pub fn escrow(&self, session: &Pubkey, owner: &Pubkey) -> Escrow {
        self.read(&escrow_pda(session, owner))
    }

    pub fn config(&self) -> RailsConfig {
        self.read(&self.config)
    }

    pub fn receipt(&self, session: &Pubkey, fill_index: u32) -> FillReceipt {
        self.read(&receipt_pda(session, fill_index))
    }

    // ── admin ───────────────────────────────────────────────────────────

    fn admin_ix(&self, signer: &Pubkey, data: Vec<u8>) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AdminConfig {
                config: self.config,
                authority: *signer,
            }
            .to_account_metas(None),
            data,
        }
    }

    pub fn set_breaker(&mut self, breaker: Breaker, engaged: bool) -> Res {
        let data = ix::SetBreaker {
            breaker,
            engaged,
            evidence_hash: [7; 32],
        }
        .data();
        let ix = self.admin_ix(&self.authority.pubkey(), data);
        let a = self.authority.insecure_clone();
        self.send(&[ix], &[&a])
    }

    pub fn set_operator_as(&mut self, signer: &Keypair, operator: Pubkey) -> Res {
        let ix = self.admin_ix(&signer.pubkey(), ix::SetOperator { operator }.data());
        self.send(&[ix], &[signer])
    }

    pub fn set_fees_as(&mut self, signer: &Keypair, fee_bps: u16, reserve_bps: u16) -> Res {
        let data = ix::SetFees {
            fee_bps,
            reserve_bps,
        }
        .data();
        let ix = self.admin_ix(&signer.pubkey(), data);
        self.send(&[ix], &[signer])
    }

    pub fn set_fees(&mut self, fee_bps: u16, reserve_bps: u16) -> Res {
        let a = self.authority.insecure_clone();
        self.set_fees_as(&a, fee_bps, reserve_bps)
    }

    // ── sessions ────────────────────────────────────────────────────────

    pub fn open_session_as(
        &mut self,
        operator: &Keypair,
        id: u64,
        schedule: (i64, i64, i64),
        bond: u64,
        max_cash: u64,
    ) -> Result<Pubkey, Fail> {
        let asset = self.asset_mint;
        let params = SessionParams {
            commit_end: schedule.0,
            reveal_end: schedule.1,
            settle_deadline: schedule.2,
            bond_amount: bond,
            max_cash,
            rofr_authority: Pubkey::default(),
        };
        self.open_session_full(operator, id, params, asset)
    }

    pub fn open_session_full(
        &mut self,
        operator: &Keypair,
        id: u64,
        params: SessionParams,
        asset_mint: Pubkey,
    ) -> Result<Pubkey, Fail> {
        let session = session_pda(&operator.pubkey(), id);
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::OpenSession {
                config: self.config,
                session,
                asset_mint,
                operator: operator.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::OpenSession {
                session_id: id,
                params,
            }
            .data(),
        };
        self.send(&[ix], &[operator]).map(|_| session)
    }

    /// Standard session opened by the authority (the initial operator).
    pub fn open_session(&mut self, id: u64) -> Pubkey {
        let a = self.authority.insecure_clone();
        self.open_session_as(
            &a,
            id,
            (COMMIT_END, REVEAL_END, SETTLE_DEADLINE),
            BOND,
            MAX_CASH,
        )
        .expect("open_session")
    }

    /// Standard session with an issuer ROFR authority.
    pub fn open_session_with_rofr(&mut self, id: u64, rofr_authority: Pubkey) -> Pubkey {
        let a = self.authority.insecure_clone();
        let params = SessionParams {
            commit_end: COMMIT_END,
            reveal_end: REVEAL_END,
            settle_deadline: SETTLE_DEADLINE,
            bond_amount: BOND,
            max_cash: MAX_CASH,
            rofr_authority,
        };
        let asset = self.asset_mint;
        self.open_session_full(&a, id, params, asset)
            .expect("open_session")
    }

    fn operator_session_ix(
        &self,
        session: &Pubkey,
        operator: &Pubkey,
        data: Vec<u8>,
    ) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::OperatorSession {
                session: *session,
                operator: *operator,
            }
            .to_account_metas(None),
            data,
        }
    }

    pub fn fail_session_as(&mut self, operator: &Keypair, session: &Pubkey) -> Res {
        let ix = self.operator_session_ix(session, &operator.pubkey(), ix::FailSession {}.data());
        self.send(&[ix], &[operator])
    }

    pub fn finish_session_as(&mut self, operator: &Keypair, session: &Pubkey) -> Res {
        let ix = self.operator_session_ix(session, &operator.pubkey(), ix::FinishSession {}.data());
        self.send(&[ix], &[operator])
    }

    pub fn finish_session(&mut self, session: &Pubkey) -> Res {
        let a = self.authority.insecure_clone();
        self.finish_session_as(&a, session)
    }

    fn operator_escrow_ix(
        &self,
        session: &Pubkey,
        owner: &Pubkey,
        operator: &Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::OperatorEscrow {
                session: *session,
                escrow: escrow_pda(session, owner),
                operator: *operator,
            }
            .to_account_metas(None),
            data: ix::MarkRevealed {}.data(),
        }
    }

    pub fn mark_revealed_as(
        &mut self,
        operator: &Keypair,
        session: &Pubkey,
        owner: &Pubkey,
    ) -> Res {
        let ix = self.operator_escrow_ix(session, owner, &operator.pubkey());
        self.send(&[ix], &[operator])
    }

    pub fn mark_revealed(&mut self, session: &Pubkey, owner: &Pubkey) -> Res {
        let a = self.authority.insecure_clone();
        self.mark_revealed_as(&a, session, owner)
    }

    // ── participants ────────────────────────────────────────────────────

    /// A bidder with `usdc` micro-USDC in a fresh token account.
    pub fn bidder(&mut self, usdc: u64) -> (Keypair, Pubkey) {
        let k = self.funded();
        let mint = self.mint;
        let t = self.token_account(&mint, &k.pubkey(), usdc);
        (k, t)
    }

    /// A participant with USDC and asset accounts, admitted by the gate,
    /// its `asset` recorded as seasoned inventory (#133).
    pub fn party(&mut self, usdc: u64, asset: u64) -> Party {
        let (key, usdc_acct) = self.bidder(usdc);
        let asset_acct = self.asset_account(&key.pubkey(), asset);
        self.admit(&key.pubkey(), asset);
        Party {
            key,
            usdc: usdc_acct,
            asset: asset_acct,
        }
    }

    /// A participant admitted as one more wallet of investor `investor_id`,
    /// holding `asset` seasoned units.
    pub fn party_of(&mut self, investor_id: [u8; 16], usdc: u64, asset: u64) -> Party {
        let (key, usdc_acct) = self.bidder(usdc);
        let asset_acct = self.asset_account(&key.pubkey(), asset);
        self.admit_investor(&key.pubkey(), investor_id, true);
        if asset > 0 {
            let now = self.now();
            self.append_lot_at(&key.pubkey(), asset, now - INVENTORY_AGE);
        }
        Party {
            key,
            usdc: usdc_acct,
            asset: asset_acct,
        }
    }

    /// A participant with USDC and asset accounts the gate has NOT
    /// admitted (no `validate`, no lots).
    pub fn stranger(&mut self, usdc: u64) -> Party {
        let (key, usdc_acct) = self.bidder(usdc);
        let asset_acct = self.asset_account(&key.pubkey(), 0);
        Party {
            key,
            usdc: usdc_acct,
            asset: asset_acct,
        }
    }

    // ── escrow ──────────────────────────────────────────────────────────

    pub fn deposit_with_mint(
        &mut self,
        session: &Pubkey,
        owner: &Keypair,
        owner_token: &Pubkey,
        mint: Pubkey,
        cash: u64,
    ) -> Res {
        let escrow = escrow_pda(session, &owner.pubkey());
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::Deposit {
                config: self.config,
                session: *session,
                escrow,
                vault: vault_pda(&escrow),
                usdc_mint: mint,
                owner_token: *owner_token,
                owner: owner.pubkey(),
                token_program: self.tp.id(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::Deposit { cash }.data(),
        };
        self.send(&[ix], &[owner])
    }

    pub fn deposit(
        &mut self,
        session: &Pubkey,
        owner: &Keypair,
        owner_token: &Pubkey,
        cash: u64,
    ) -> Res {
        let mint = self.mint;
        self.deposit_with_mint(session, owner, owner_token, mint, cash)
    }

    pub fn open_asset_vault_ix(&self, session: &Pubkey, owner: &Pubkey) -> Instruction {
        let escrow = escrow_pda(session, owner);
        Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::OpenAssetVault {
                config: self.config,
                session: *session,
                escrow,
                asset_vault: asset_vault_pda(&escrow),
                asset_mint: self.asset_mint,
                owner: *owner,
                asset_token_program: self.asset_tp.id(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::OpenAssetVault {}.data(),
        }
    }

    pub fn open_asset_vault(&mut self, session: &Pubkey, who: &Party) -> Res {
        let ix = self.open_asset_vault_ix(session, &who.key.pubkey());
        self.send(&[ix], &[&who.key])
    }

    /// Open the asset vault and deposit in one transaction (unfrozen mint).
    pub fn deposit_ask(&mut self, session: &Pubkey, who: &Party, qty: u64) -> Res {
        let open = self.open_asset_vault_ix(session, &who.key.pubkey());
        let deposit = self.deposit_ask_ix(session, who, qty);
        self.send(&[open, deposit], &[&who.key])
    }

    /// `deposit_ask` alone, into a vault opened earlier.
    pub fn deposit_ask_only(&mut self, session: &Pubkey, who: &Party, qty: u64) -> Res {
        let ix = self.deposit_ask_ix(session, who, qty);
        self.send(&[ix], &[&who.key])
    }

    fn deposit_ask_ix(&self, session: &Pubkey, who: &Party, qty: u64) -> Instruction {
        let escrow = escrow_pda(session, &who.key.pubkey());
        Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::DepositAsk {
                config: self.config,
                session: *session,
                escrow,
                vault: vault_pda(&escrow),
                asset_vault: asset_vault_pda(&escrow),
                usdc_mint: self.mint,
                asset_mint: self.asset_mint,
                owner_usdc: who.usdc,
                owner_asset: who.asset,
                owner: who.key.pubkey(),
                token_program: self.tp.id(),
                asset_token_program: self.asset_tp.id(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::DepositAsk { qty }.data(),
        }
    }

    pub fn forfeit_bond(&mut self, caller: &Keypair, session: &Pubkey, owner: &Pubkey) -> Res {
        let escrow = escrow_pda(session, owner);
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::ForfeitBond {
                caller: caller.pubkey(),
                session: *session,
                escrow,
                vault: vault_pda(&escrow),
                reserve: self.reserve,
                usdc_mint: self.mint,
                token_program: self.tp.id(),
            }
            .to_account_metas(None),
            data: ix::ForfeitBond {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    /// Refund with explicit destination + reserve (for spoofing tests).
    pub fn refund_to(
        &mut self,
        caller: &Keypair,
        session: &Pubkey,
        owner: &Pubkey,
        owner_token: &Pubkey,
        reserve: &Pubkey,
    ) -> Res {
        let escrow = escrow_pda(session, owner);
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::Refund {
                caller: caller.pubkey(),
                session: *session,
                escrow,
                vault: vault_pda(&escrow),
                owner: *owner,
                owner_token: *owner_token,
                reserve: *reserve,
                usdc_mint: self.mint,
                token_program: self.tp.id(),
            }
            .to_account_metas(None),
            data: ix::Refund {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    pub fn refund(
        &mut self,
        caller: &Keypair,
        session: &Pubkey,
        owner: &Pubkey,
        owner_token: &Pubkey,
    ) -> Res {
        let reserve = self.reserve;
        self.refund_to(caller, session, owner, owner_token, &reserve)
    }

    pub fn refund_ask(&mut self, caller: &Keypair, session: &Pubkey, who: &Party) -> Res {
        let escrow = escrow_pda(session, &who.key.pubkey());
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::RefundAsk {
                caller: caller.pubkey(),
                session: *session,
                escrow,
                vault: vault_pda(&escrow),
                asset_vault: asset_vault_pda(&escrow),
                owner: who.key.pubkey(),
                owner_usdc: who.usdc,
                owner_asset: who.asset,
                reserve: self.reserve,
                usdc_mint: self.mint,
                asset_mint: self.asset_mint,
                token_program: self.tp.id(),
                asset_token_program: self.asset_tp.id(),
            }
            .to_account_metas(None),
            data: ix::RefundAsk {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    // ── settlement ──────────────────────────────────────────────────────

    pub fn fill(&self, session: &Pubkey, buyer: &Party, seller: &Party) -> Fill {
        Fill {
            session: *session,
            buyer_owner: buyer.key.pubkey(),
            seller_owner: seller.key.pubkey(),
            seller_usdc: seller.usdc,
            buyer_asset: buyer.asset,
            fee_receiver: self.fee,
            reserve: self.reserve,
        }
    }

    /// The `settle_fill` instruction for `f`, gate accounts included.
    pub fn settle_ix(
        &self,
        operator: &Pubkey,
        f: &Fill,
        fill_index: u32,
        qty: u64,
        price: u64,
    ) -> Instruction {
        let buyer_escrow = escrow_pda(&f.session, &f.buyer_owner);
        let seller_escrow = escrow_pda(&f.session, &f.seller_owner);
        Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::SettleFill {
                config: self.config,
                session: f.session,
                operator: *operator,
                receipt: receipt_pda(&f.session, fill_index),
                buyer_escrow,
                buyer_vault: vault_pda(&buyer_escrow),
                seller_escrow,
                seller_asset_vault: asset_vault_pda(&seller_escrow),
                seller_usdc: f.seller_usdc,
                buyer_asset: f.buyer_asset,
                fee_receiver: f.fee_receiver,
                reserve: f.reserve,
                usdc_mint: self.mint,
                asset_mint: self.asset_mint,
                token_program: self.tp.id(),
                asset_token_program: self.asset_tp.id(),
                system_program: anchor_lang::system_program::ID,
                gate: acc::FillGate {
                    gate_program: GATE_ID,
                    gate_authority: gate_settle_pda(),
                    gate_config: self.gate_config(),
                    gate_policy: self.gate_policy(),
                    seller_block: self.block_entry(&f.seller_owner),
                    buyer_block: self.block_entry(&f.buyer_owner),
                    seller_lots: self.lots_pda(&f.seller_owner),
                    buyer_lots: self.lots_pda(&f.buyer_owner),
                    buyer_link: self.link_pda(&f.buyer_owner),
                    buyer_identity: self.identity_att(&f.buyer_owner),
                    buyer_accreditation: self.accreditation_att(&f.buyer_owner),
                },
            }
            .to_account_metas(None),
            data: ix::SettleFill {
                fill_index,
                qty,
                price,
                disclosure_hash: [9; 32],
            }
            .data(),
        }
    }

    pub fn settle_as(
        &mut self,
        operator: &Keypair,
        f: &Fill,
        fill_index: u32,
        qty: u64,
        price: u64,
    ) -> Res {
        let ix = self.settle_ix(&operator.pubkey(), f, fill_index, qty, price);
        self.send(&[ix], &[operator])
    }

    pub fn settle(&mut self, f: &Fill, fill_index: u32, qty: u64, price: u64) -> Res {
        let a = self.authority.insecure_clone();
        self.settle_as(&a, f, fill_index, qty, price)
    }

    pub fn settle_rofr(
        &mut self,
        issuer: &Party,
        session: &Pubkey,
        seller: &Party,
        fill_index: u32,
        qty: u64,
        price: u64,
    ) -> Res {
        let seller_escrow = escrow_pda(session, &seller.key.pubkey());
        let op = self.authority.insecure_clone();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::SettleRofrFill {
                config: self.config,
                session: *session,
                operator: op.pubkey(),
                issuer: issuer.key.pubkey(),
                receipt: receipt_pda(session, fill_index),
                seller_escrow,
                seller_asset_vault: asset_vault_pda(&seller_escrow),
                seller_usdc: seller.usdc,
                issuer_usdc: issuer.usdc,
                issuer_asset: issuer.asset,
                fee_receiver: self.fee,
                reserve: self.reserve,
                usdc_mint: self.mint,
                asset_mint: self.asset_mint,
                token_program: self.tp.id(),
                asset_token_program: self.asset_tp.id(),
                system_program: anchor_lang::system_program::ID,
                gate: acc::RofrGate {
                    gate_program: GATE_ID,
                    gate_authority: gate_settle_pda(),
                    gate_config: self.gate_config(),
                    seller_block: self.block_entry(&seller.key.pubkey()),
                    issuer_block: self.block_entry(&issuer.key.pubkey()),
                    seller_lots: self.lots_pda(&seller.key.pubkey()),
                },
            }
            .to_account_metas(None),
            data: ix::SettleRofrFill {
                fill_index,
                qty,
                price,
                disclosure_hash: [9; 32],
            }
            .data(),
        };
        self.send(&[ix], &[&op, &issuer.key])
    }

    // ── R3 · payout registry ────────────────────────────────────────────

    pub fn set_registrar(&mut self, registrar: Pubkey) -> Res {
        let ix = self.admin_ix(
            &self.authority.pubkey(),
            ix::SetRegistrar { registrar }.data(),
        );
        let a = self.authority.insecure_clone();
        self.send(&[ix], &[&a])
    }

    pub fn register_as(
        &mut self,
        registrar: &Keypair,
        owner: &Pubkey,
        token_account: &Pubkey,
        expires_at: i64,
    ) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::RegisterDestination {
                config: self.config,
                destination: dest_pda(owner),
                owner: *owner,
                token_account: *token_account,
                registrar: registrar.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::RegisterDestination {
                attestation_ref_hash: [0xAB; 32],
                expires_at,
            }
            .data(),
        };
        self.send(&[ix], &[registrar])
    }

    /// Register `p` (its USDC account) with the authority as registrar.
    pub fn register(&mut self, p: &Party, expires_at: i64) -> Res {
        let a = self.authority.insecure_clone();
        self.register_as(&a, &p.key.pubkey(), &p.usdc, expires_at)
    }

    pub fn renew(&mut self, owner: &Pubkey, token_account: &Pubkey, expires_at: i64) -> Res {
        let a = self.authority.insecure_clone();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::RenewDestination {
                config: self.config,
                destination: dest_pda(owner),
                token_account: *token_account,
                registrar: a.pubkey(),
            }
            .to_account_metas(None),
            data: ix::RenewDestination {
                attestation_ref_hash: [0xCD; 32],
                expires_at,
            }
            .data(),
        };
        self.send(&[ix], &[&a])
    }

    pub fn revoke_as(&mut self, registrar: &Keypair, owner: &Pubkey) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::RevokeDestination {
                config: self.config,
                destination: dest_pda(owner),
                registrar: registrar.pubkey(),
            }
            .to_account_metas(None),
            data: ix::RevokeDestination {}.data(),
        };
        self.send(&[ix], &[registrar])
    }

    pub fn revoke(&mut self, owner: &Pubkey) -> Res {
        let a = self.authority.insecure_clone();
        self.revoke_as(&a, owner)
    }

    pub fn destination(&self, owner: &Pubkey) -> PayoutDestination {
        self.read(&dest_pda(owner))
    }

    // ── R3 · facility ───────────────────────────────────────────────────

    pub fn open_facility_as(
        &mut self,
        operator: &Keypair,
        id: u64,
        rate_bps: u64,
    ) -> Result<Pubkey, Fail> {
        let facility = facility_pda(&operator.pubkey(), id);
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::OpenFacility {
                config: self.config,
                facility,
                vault: facility_vault_pda(&facility),
                usdc_mint: self.mint,
                operator: operator.pubkey(),
                token_program: self.tp.id(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::OpenFacility {
                facility_id: id,
                rate_bps,
            }
            .data(),
        };
        self.send(&[ix], &[operator]).map(|_| facility)
    }

    pub fn open_facility(&mut self, id: u64, rate_bps: u64) -> Pubkey {
        let a = self.authority.insecure_clone();
        self.open_facility_as(&a, id, rate_bps)
            .expect("open_facility")
    }

    pub fn fund_facility(&mut self, facility: &Pubkey, funder: &Party, amount: u64) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::FundFacility {
                facility: *facility,
                vault: facility_vault_pda(facility),
                usdc_mint: self.mint,
                funder_token: funder.usdc,
                funder: funder.key.pubkey(),
                token_program: self.tp.id(),
            }
            .to_account_metas(None),
            data: ix::FundFacility { amount }.data(),
        };
        self.send(&[ix], &[&funder.key])
    }

    /// Advance with explicit destination accounts (for spoofing tests),
    /// sized against a collateral mark of `2 × amount` at the 50% H1
    /// ceiling, so the rate bound (#94) is exactly met and never in the way.
    pub fn advance_to(
        &mut self,
        curator: &Keypair,
        facility: &Pubkey,
        borrower: &Pubkey,
        destination: &Pubkey,
        destination_token: &Pubkey,
        amount: u64,
    ) -> Res {
        self.advance_full(
            curator,
            facility,
            borrower,
            destination,
            destination_token,
            amount,
            amount.saturating_mul(2),
            MAX_ADVANCE_RATE_BPS,
        )
    }

    /// Advance with explicit collateral mark and advance rate (#94).
    #[allow(clippy::too_many_arguments)]
    pub fn advance_full(
        &mut self,
        curator: &Keypair,
        facility: &Pubkey,
        borrower: &Pubkey,
        destination: &Pubkey,
        destination_token: &Pubkey,
        amount: u64,
        collateral_value: u64,
        advance_rate_bps: u64,
    ) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AdvanceFunds {
                config: self.config,
                facility: *facility,
                advance: advance_pda(facility, borrower),
                vault: facility_vault_pda(facility),
                borrower: *borrower,
                destination: *destination,
                destination_token: *destination_token,
                usdc_mint: self.mint,
                curator: curator.pubkey(),
                token_program: self.tp.id(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::AdvanceFunds {
                amount,
                collateral_ref_hash: [0xC1; 32],
                collateral_value,
                advance_rate_bps,
            }
            .data(),
        };
        self.send(&[ix], &[curator])
    }

    pub fn advance(&mut self, facility: &Pubkey, borrower: &Party, amount: u64) -> Res {
        let a = self.authority.insecure_clone();
        let owner = borrower.key.pubkey();
        self.advance_to(
            &a,
            facility,
            &owner,
            &dest_pda(&owner),
            &borrower.usdc,
            amount,
        )
    }

    pub fn repay(
        &mut self,
        facility: &Pubkey,
        borrower: &Pubkey,
        payer: &Party,
        amount: u64,
    ) -> Res {
        let curator = self.read::<Facility>(facility).curator;
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::Repay {
                facility: *facility,
                advance: advance_pda(facility, borrower),
                vault: facility_vault_pda(facility),
                curator,
                payer_token: payer.usdc,
                payer: payer.key.pubkey(),
                usdc_mint: self.mint,
                token_program: self.tp.id(),
            }
            .to_account_metas(None),
            data: ix::Repay { amount }.data(),
        };
        self.send(&[ix], &[&payer.key])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sweep(
        &mut self,
        facility: &Pubkey,
        borrower: &Pubkey,
        proceeds: &Party,
        holder_destination: &Pubkey,
        holder_token: &Pubkey,
        amount: u64,
    ) -> Res {
        let curator = self.read::<Facility>(facility).curator;
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::SweepProceeds {
                facility: *facility,
                advance: advance_pda(facility, borrower),
                vault: facility_vault_pda(facility),
                curator,
                proceeds_token: proceeds.usdc,
                proceeds_authority: proceeds.key.pubkey(),
                holder_destination: *holder_destination,
                holder_token: *holder_token,
                usdc_mint: self.mint,
                token_program: self.tp.id(),
            }
            .to_account_metas(None),
            data: ix::SweepProceeds { amount }.data(),
        };
        self.send(&[ix], &[&proceeds.key])
    }

    pub fn facility(&self, facility: &Pubkey) -> Facility {
        self.read(facility)
    }

    pub fn advance_state(&self, facility: &Pubkey, borrower: &Pubkey) -> Advance {
        self.read(&advance_pda(facility, borrower))
    }

    // ── R3 · coupons ────────────────────────────────────────────────────

    /// Distribute from `source` (a USDC account the authority/operator
    /// owns) to explicit destination accounts.
    #[allow(clippy::too_many_arguments)]
    pub fn distribute(
        &mut self,
        source: &Pubkey,
        period: u64,
        cash: u64,
        params: CouponParams,
        senior: (&Pubkey, &Pubkey),
        junior: (&Pubkey, &Pubkey),
    ) -> Res {
        let a = self.authority.insecure_clone();
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::DistributeCoupon {
                config: self.config,
                receipt: coupon_pda(&a.pubkey(), period),
                source: *source,
                fee_receiver: self.fee,
                reserve: self.reserve,
                senior_destination: *senior.0,
                senior_token: *senior.1,
                junior_destination: *junior.0,
                junior_token: *junior.1,
                usdc_mint: self.mint,
                operator: a.pubkey(),
                token_program: self.tp.id(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::DistributeCoupon {
                period_id: period,
                cash,
                params,
                period_ref_hash: [0x77; 32],
            }
            .data(),
        };
        self.send(&[ix], &[&a])
    }

    /// Sum of balances over `accounts` — the conservation probe.
    pub fn total(&self, accounts: &[Pubkey]) -> u64 {
        accounts.iter().map(|a| self.bal(a)).sum()
    }
}
