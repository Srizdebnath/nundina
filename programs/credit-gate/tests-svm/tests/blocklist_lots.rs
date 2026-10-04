//! #122: the CG-P5 block-list and the CG-P3 lot ledger, against the real
//! SBF build of the current source (build.rs).
//!
//! - Only the gate-config authority edits the list; a blocked wallet fails
//!   `check_blocklist`, cannot open a lot ledger and cannot receive a lot,
//!   and unblocking restores all three (precedence on every path).
//! - A config that points at a list the gate can't read fails closed.
//! - Lots season from acquisition by the config's `seasoning_seconds`;
//!   splits keep seasoning; `merge_expired` is permissionless and moves
//!   only expired quantity into FREE; every change emits one `LotEvent`.

mod common;

use anchor_lang::prelude::{Clock, Pubkey};
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, AnchorDeserialize, Discriminator};
use anchor_lang::{InstructionData, ToAccountMetas};
use base64::Engine;
use common::{deploy, root_pda, seed_root, PROGRAM_ID};
use credit_gate::{
    accounts as acc, instruction as ix, BlockEntry, BlockList, BlocklistUpdated, GateError,
    HolderLots, LotEvent, LotSource, MarketConfig, MAX_LOTS,
};
use litesvm::LiteSVM;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

const DAY: i64 = 86_400;
const SEASONING: i64 = 7 * DAY;

fn config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[MarketConfig::SEED, mint.as_ref()], &PROGRAM_ID).0
}
fn blocklist_pda(mint: &Pubkey) -> Pubkey {
    BlockList::address(mint)
}
fn entry_pda(mint: &Pubkey, wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[BlockEntry::SEED, mint.as_ref(), wallet.as_ref()],
        &PROGRAM_ID,
    )
    .0
}
fn lots_pda(mint: &Pubkey, holder: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[HolderLots::SEED, mint.as_ref(), holder.as_ref()],
        &PROGRAM_ID,
    )
    .0
}
fn gate(e: GateError) -> u32 {
    u32::from(e)
}

struct Env {
    svm: LiteSVM,
    auth: Keypair,
    mint: Pubkey,
}

impl Env {
    /// Root seeded, one market configured with 7-day seasoning, no list yet.
    fn new() -> Env {
        let auth = Keypair::new();
        let mut svm = deploy(&auth);
        seed_root(&mut svm, &auth);
        let mut env = Env {
            svm,
            auth,
            mint: Pubkey::new_unique(),
        };
        let init = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeConfig {
                config: config_pda(&env.mint),
                root: root_pda(),
                mint: env.mint,
                authority: env.auth.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::InitializeConfig {
                policy_class: 1,
                holder_cap: 8,
                seasoning_seconds: SEASONING,
                settlement_horizon_seconds: DAY,
                attesters: vec![],
                identity_schema: Pubkey::new_unique(),
                accreditation_schema: Pubkey::new_unique(),
            }
            .data(),
        };
        let auth = env.auth.insecure_clone();
        env.send(init, &auth).unwrap();
        env.set_time(1_000_000);
        env
    }

    fn funded(&mut self) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), 1_000_000_000).unwrap();
        k
    }

    fn set_time(&mut self, t: i64) {
        let mut c: Clock = self.svm.get_sysvar();
        c.unix_timestamp = t;
        self.svm.set_sysvar(&c);
    }

    fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    /// Send; on success return the program's `Program data:` payloads.
    fn send(&mut self, ix: Instruction, signer: &Keypair) -> Result<Vec<Vec<u8>>, u32> {
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&signer.pubkey()),
            &[signer],
            self.svm.latest_blockhash(),
        );
        let out = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        match out {
            Ok(meta) => Ok(meta
                .logs
                .iter()
                .filter_map(|l| l.strip_prefix("Program data: "))
                .filter_map(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
                .collect()),
            Err(f) => match f.err {
                TransactionError::InstructionError(_, InstructionError::Custom(c)) => Err(c),
                other => panic!("unexpected failure {other:?} | {:?}", f.meta.logs),
            },
        }
    }

    fn auth(&self) -> Keypair {
        self.auth.insecure_clone()
    }

    fn init_blocklist(&mut self, signer: &Keypair) -> Result<Vec<Vec<u8>>, u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::InitializeBlocklist {
                config: config_pda(&self.mint),
                blocklist: blocklist_pda(&self.mint),
                mint: self.mint,
                gate_config_authority: signer.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::InitializeBlocklist {}.data(),
        };
        self.send(ix, signer)
    }

    fn block(&mut self, signer: &Keypair, wallet: Pubkey) -> Result<Vec<Vec<u8>>, u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::BlockWallet {
                config: config_pda(&self.mint),
                blocklist: blocklist_pda(&self.mint),
                entry: entry_pda(&self.mint, &wallet),
                mint: self.mint,
                gate_config_authority: signer.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::BlockWallet {
                wallet,
                reason_hash: [7; 32],
            }
            .data(),
        };
        self.send(ix, signer)
    }

    fn unblock(&mut self, signer: &Keypair, wallet: Pubkey) -> Result<Vec<Vec<u8>>, u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::UnblockWallet {
                config: config_pda(&self.mint),
                blocklist: blocklist_pda(&self.mint),
                entry: entry_pda(&self.mint, &wallet),
                mint: self.mint,
                gate_config_authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix::UnblockWallet { wallet }.data(),
        };
        self.send(ix, signer)
    }

    fn check(&mut self, wallet: Pubkey) -> Result<(), u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::CheckBlocklist {
                config: config_pda(&self.mint),
                entry: entry_pda(&self.mint, &wallet),
                mint: self.mint,
            }
            .to_account_metas(None),
            data: ix::CheckBlocklist { _wallet: wallet }.data(),
        };
        let payer = self.funded();
        self.send(ix, &payer).map(|_| ())
    }

    fn open_lots(&mut self, holder: Pubkey) -> Result<(), u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::OpenHolderLots {
                config: config_pda(&self.mint),
                lots: lots_pda(&self.mint, &holder),
                block_entry: entry_pda(&self.mint, &holder),
                mint: self.mint,
                payer: self.auth.pubkey(),
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: ix::OpenHolderLots { holder }.data(),
        };
        let auth = self.auth();
        self.send(ix, &auth).map(|_| ())
    }

    fn append(&mut self, signer: &Keypair, holder: Pubkey, qty: u64) -> Result<Vec<Vec<u8>>, u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::AppendLot {
                config: config_pda(&self.mint),
                lots: lots_pda(&self.mint, &holder),
                block_entry: entry_pda(&self.mint, &holder),
                mint: self.mint,
                gate_config_authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix::AppendLot {
                qty,
                source: LotSource::AuctionSettle,
                affiliate: false,
                exemption_path: 2,
            }
            .data(),
        };
        self.send(ix, signer)
    }

    fn split(
        &mut self,
        signer: &Keypair,
        holder: Pubkey,
        lot_id: u32,
        qty: u64,
    ) -> Result<Vec<Vec<u8>>, u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::SplitLot {
                config: config_pda(&self.mint),
                lots: lots_pda(&self.mint, &holder),
                mint: self.mint,
                gate_config_authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix::SplitLot { lot_id, qty }.data(),
        };
        self.send(ix, signer)
    }

    fn merge(&mut self, signer: &Keypair, holder: Pubkey) -> Result<Vec<Vec<u8>>, u32> {
        let ix = Instruction {
            program_id: PROGRAM_ID,
            accounts: acc::MergeExpired {
                lots: lots_pda(&self.mint, &holder),
                mint: self.mint,
            }
            .to_account_metas(None),
            data: ix::MergeExpired {}.data(),
        };
        self.send(ix, signer)
    }

    fn lots(&self, holder: &Pubkey) -> HolderLots {
        let a = self.svm.get_account(&lots_pda(&self.mint, holder)).unwrap();
        HolderLots::try_deserialize(&mut a.data.as_slice()).unwrap()
    }

    fn config(&self) -> MarketConfig {
        let a = self.svm.get_account(&config_pda(&self.mint)).unwrap();
        MarketConfig::try_deserialize(&mut a.data.as_slice()).unwrap()
    }

    fn exists(&self, k: &Pubkey) -> bool {
        self.svm.get_account(k).is_some_and(|a| !a.data.is_empty())
    }
}

fn events<E: AnchorDeserialize + Discriminator>(data: &[Vec<u8>]) -> Vec<E> {
    data.iter()
        .filter(|d| d.starts_with(E::DISCRIMINATOR))
        .map(|d| E::try_from_slice(&d[E::DISCRIMINATOR.len()..]).unwrap())
        .collect()
}

#[test]
fn no_list_configured_screens_clear() {
    let mut env = Env::new();
    assert_eq!(env.config().blocklist, None);
    let w = Pubkey::new_unique();
    assert_eq!(env.check(w), Ok(()));
    assert_eq!(env.open_lots(w), Ok(()));
}

#[test]
fn only_the_gate_authority_edits_the_list() {
    let mut env = Env::new();
    let mallory = env.funded();
    let w = Pubkey::new_unique();
    assert_eq!(
        env.init_blocklist(&mallory),
        Err(gate(GateError::Unauthorized))
    );
    let auth = env.auth();
    env.init_blocklist(&auth).unwrap();
    assert_eq!(env.config().blocklist, Some(blocklist_pda(&env.mint)));
    assert_eq!(env.block(&mallory, w), Err(gate(GateError::Unauthorized)));
    env.block(&auth, w).unwrap();
    assert_eq!(env.unblock(&mallory, w), Err(gate(GateError::Unauthorized)));
    assert!(env.exists(&entry_pda(&env.mint, &w)));
}

#[test]
fn a_block_wins_on_every_path_and_unblocking_restores_it() {
    let mut env = Env::new();
    let auth = env.auth();
    env.init_blocklist(&auth).unwrap();
    let (blocked, held) = (Pubkey::new_unique(), Pubkey::new_unique());
    env.open_lots(held).unwrap();

    let ev = env.block(&auth, blocked).unwrap();
    let ev: Vec<BlocklistUpdated> = events(&ev);
    assert_eq!(ev.len(), 1);
    assert!(ev[0].blocked && ev[0].wallet == blocked && ev[0].entries == 1);
    // Blocking twice is an error, not a silent no-op.
    assert!(env.block(&auth, blocked).is_err());

    let bl = Err(gate(GateError::Blocklisted));
    assert_eq!(env.check(blocked), bl);
    assert_eq!(env.open_lots(blocked), bl);
    // A holder blocked *after* opening a ledger can't receive another lot.
    env.block(&auth, held).unwrap();
    assert_eq!(env.append(&auth, held, 10).map(|_| ()), bl);
    // Others are untouched.
    assert_eq!(env.check(Pubkey::new_unique()), Ok(()));

    let ev: Vec<BlocklistUpdated> = events(&env.unblock(&auth, held).unwrap());
    assert!(!ev[0].blocked && ev[0].reason_hash == [7; 32] && ev[0].entries == 1);
    assert!(!env.exists(&entry_pda(&env.mint, &held)));
    assert_eq!(env.check(held), Ok(()));
    env.append(&auth, held, 10).unwrap();
}

#[test]
fn a_list_the_gate_cannot_read_fails_closed() {
    let mut env = Env::new();
    let auth = env.auth();
    // Point the config at some other account: every screen must refuse.
    let foreign = Pubkey::new_unique();
    let ix = Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::UpdateConfig {
            config: config_pda(&env.mint),
            mint: env.mint,
            gate_config_authority: auth.pubkey(),
        }
        .to_account_metas(None),
        data: ix::UpdateConfig {
            policy_class: None,
            holder_cap: None,
            seasoning_seconds: None,
            settlement_horizon_seconds: None,
            attester_add: None,
            attester_remove: None,
            blocklist_set: Some(foreign),
        }
        .data(),
    };
    env.send(ix, &auth).unwrap();
    let w = Pubkey::new_unique();
    let unreadable = Err(gate(GateError::BlocklistUnreadable));
    assert_eq!(env.check(w), unreadable);
    assert_eq!(env.open_lots(w), unreadable);
    // initialize_blocklist re-points it at the real header and clears that.
    env.init_blocklist(&auth).unwrap();
    assert_eq!(env.check(w), Ok(()));
}

#[test]
fn lots_season_from_acquisition_and_merge_permissionlessly() {
    let mut env = Env::new();
    let auth = env.auth();
    let holder = Pubkey::new_unique();
    env.open_lots(holder).unwrap();
    let t0 = env.now();

    let ev: Vec<LotEvent> = events(&env.append(&auth, holder, 500).unwrap());
    assert_eq!(ev.len(), 1, "one LotEvent per change (CG-P7)");
    assert_eq!((ev[0].kind, ev[0].lot_id, ev[0].qty), (0, 0, 500));
    assert_eq!(ev[0].restriction_expiry, t0 + SEASONING);

    // Only the gate authority appends or splits.
    let mallory = env.funded();
    assert_eq!(
        env.append(&mallory, holder, 1),
        Err(gate(GateError::Unauthorized))
    );
    assert_eq!(
        env.split(&mallory, holder, 0, 1),
        Err(gate(GateError::Unauthorized))
    );

    env.set_time(t0 + DAY);
    env.append(&auth, holder, 300).unwrap(); // lot 1, expires t0 + DAY + SEASONING
    let ev: Vec<LotEvent> = events(&env.split(&auth, holder, 0, 200).unwrap());
    assert_eq!((ev[0].kind, ev[0].lot_id, ev[0].qty), (1, 2, 200));
    assert_eq!(
        ev[0].restriction_expiry,
        t0 + SEASONING,
        "a split keeps seasoning"
    );
    let l = env.lots(&holder);
    assert_eq!(
        l.live()
            .iter()
            .map(|l| (l.lot_id, l.qty))
            .collect::<Vec<_>>(),
        vec![(0, 300), (1, 300), (2, 200)]
    );
    assert_eq!(l.total().unwrap(), 800);

    // Before seasoning: nothing to merge, nothing sellable.
    let crank = env.funded();
    assert_eq!(
        env.merge(&crank, holder),
        Err(gate(GateError::NothingExpired))
    );
    assert_eq!(env.lots(&holder).sellable(env.now()).unwrap(), 0);

    // At lot 0/2's expiry: anyone merges exactly those into FREE.
    env.set_time(t0 + SEASONING);
    assert_eq!(env.lots(&holder).sellable(env.now()).unwrap(), 500);
    let ev: Vec<LotEvent> = events(&env.merge(&crank, holder).unwrap());
    assert_eq!((ev[0].kind, ev[0].qty, ev[0].free_qty), (2, 500, 500));
    let l = env.lots(&holder);
    assert_eq!((l.free_qty, l.lot_count, l.lots[0].lot_id), (500, 1, 1));
    assert_eq!(
        l.total().unwrap(),
        800,
        "merging moves quantity, never changes it"
    );
}

#[test]
fn a_full_ledger_rejects_only_while_nothing_has_seasoned() {
    let mut env = Env::new();
    let auth = env.auth();
    let holder = Pubkey::new_unique();
    env.open_lots(holder).unwrap();
    let t0 = env.now();
    for i in 0..MAX_LOTS as i64 {
        env.set_time(t0 + i);
        env.append(&auth, holder, 1).unwrap();
    }
    assert_eq!(
        env.append(&auth, holder, 1),
        Err(gate(GateError::LotLedgerFull))
    );
    // Once the first lot seasons, the next append compacts it into FREE.
    env.set_time(t0 + SEASONING);
    env.append(&auth, holder, 1).unwrap();
    let l = env.lots(&holder);
    assert_eq!((l.free_qty, usize::from(l.lot_count)), (1, MAX_LOTS));
    assert_eq!(l.total().unwrap(), MAX_LOTS as u64 + 1);
}

#[test]
fn ledger_and_entry_addresses_cannot_be_substituted() {
    let mut env = Env::new();
    let auth = env.auth();
    env.init_blocklist(&auth).unwrap();
    let (blocked, clean) = (Pubkey::new_unique(), Pubkey::new_unique());
    env.block(&auth, blocked).unwrap();
    // Screening `blocked` while passing `clean`'s (empty) entry address
    // fails the seeds constraint instead of passing the screen.
    let ix = Instruction {
        program_id: PROGRAM_ID,
        accounts: acc::CheckBlocklist {
            config: config_pda(&env.mint),
            entry: entry_pda(&env.mint, &clean),
            mint: env.mint,
        }
        .to_account_metas(None),
        data: ix::CheckBlocklist { _wallet: blocked }.data(),
    };
    let payer = env.funded();
    assert_eq!(
        env.send(ix, &payer).map(|_| ()),
        Err(u32::from(anchor_lang::error::ErrorCode::ConstraintSeeds))
    );
}
