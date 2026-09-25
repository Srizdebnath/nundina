//! Shared LiteSVM harness: loads the real `anchor build` artifact as an
//! upgradeable program, fabricates a 6-dp settlement mint under either token
//! program, and wraps every rails instruction.

#![allow(dead_code)]

use anchor_lang::prelude::{Clock, Pubkey};
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::token_2022::spl_token_2022;
use litesvm::types::TransactionMetadata;
use litesvm::LiteSVM;
use payment_rails::{
    accounts as acc, instruction as ix, Breaker, CashSession, Escrow, RailsConfig, RailsError,
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

const LOADER_V3: Pubkey = anchor_lang::solana_program::bpf_loader_upgradeable::ID;

/// Which token program owns the settlement mint.
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

pub struct Env {
    pub svm: LiteSVM,
    pub tp: Tp,
    pub authority: Keypair,
    pub mint: Pubkey,
    pub fee: Pubkey,
    pub reserve: Pubkey,
    pub config: Pubkey,
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
            authority,
            mint: Pubkey::default(),
            fee: Pubkey::default(),
            reserve: Pubkey::default(),
            config: pda(&[RailsConfig::SEED]),
        };
        env.set_time(T0);
        env.mint = env.new_mint(6);
        let auth = env.authority.pubkey();
        env.fee = env.token_account(&env.mint.clone(), &auth, 0);
        env.reserve = env.token_account(&env.mint.clone(), &auth, 0);

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
        self.put(key, data);
        key
    }

    pub fn token_account(&mut self, mint: &Pubkey, owner: &Pubkey, amount: u64) -> Pubkey {
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
        self.put(key, data);
        key
    }

    fn put(&mut self, key: Pubkey, data: Vec<u8>) {
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                key,
                Account {
                    lamports,
                    data,
                    owner: self.tp.id(),
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
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

    /// Plain token transfer by `from_owner` (used to grief vaults).
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

    // ── account readers ─────────────────────────────────────────────────

    pub fn read<T: AccountDeserialize>(&self, key: &Pubkey) -> T {
        let a = self.svm.get_account(key).expect("account exists");
        T::try_deserialize(&mut a.data.as_slice()).expect("deserialize")
    }

    pub fn session(&self, key: &Pubkey) -> CashSession {
        self.read(key)
    }

    pub fn config(&self) -> RailsConfig {
        self.read(&self.config)
    }

    // ── rails instructions ──────────────────────────────────────────────

    pub fn set_breaker(&mut self, breaker: Breaker, engaged: bool) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AdminConfig {
                config: self.config,
                authority: self.authority.pubkey(),
            }
            .to_account_metas(None),
            data: ix::SetBreaker {
                breaker,
                engaged,
                evidence_hash: [7; 32],
            }
            .data(),
        };
        let a = self.authority.insecure_clone();
        self.send(&[ix], &[&a])
    }

    pub fn set_operator_as(&mut self, signer: &Keypair, operator: Pubkey) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AdminConfig {
                config: self.config,
                authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix::SetOperator { operator }.data(),
        };
        self.send(&[ix], &[signer])
    }

    pub fn open_session_as(
        &mut self,
        operator: &Keypair,
        id: u64,
        schedule: (i64, i64, i64),
        bond: u64,
        max_cash: u64,
    ) -> Result<Pubkey, Fail> {
        let session = session_pda(&operator.pubkey(), id);
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::OpenSession {
                config: self.config,
                session,
                operator: operator.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::OpenSession {
                session_id: id,
                commit_end: schedule.0,
                reveal_end: schedule.1,
                settle_deadline: schedule.2,
                bond_amount: bond,
                max_cash,
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

    /// A bidder with `usdc` micro-USDC in a fresh token account.
    pub fn bidder(&mut self, usdc: u64) -> (Keypair, Pubkey) {
        let k = self.funded();
        let mint = self.mint;
        let t = self.token_account(&mint, &k.pubkey(), usdc);
        (k, t)
    }

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

    pub fn fail_session_as(&mut self, operator: &Keypair, session: &Pubkey) -> Res {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::FailSession {
                session: *session,
                operator: operator.pubkey(),
            }
            .to_account_metas(None),
            data: ix::FailSession {}.data(),
        };
        self.send(&[ix], &[operator])
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

    /// Sum of balances over `accounts` — the conservation probe.
    pub fn total(&self, accounts: &[Pubkey]) -> u64 {
        accounts.iter().map(|a| self.bal(a)).sum()
    }
}
