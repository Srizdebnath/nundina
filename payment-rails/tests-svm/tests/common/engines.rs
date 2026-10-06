//! The engine programs (#136–#140) next to the rails and the gate: loaders
//! for their root-workspace `anchor build` artifacts and instruction
//! wrappers shared by the engine suites.

use super::{
    asset_vault_pda, escrow_pda, receipt_pda, session_pda, vault_pda, Env, Fill, Party, Res,
};
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use auction_engine::{
    accounts as aacc, instruction as aix, operator_pda, print_pda, Session as AuctionSession,
    SessionParams as AuctionParams, Side, VENUE_SEED,
};
use mark_engine::{
    accounts as macc, instruction as mix, Feed, FeedParams, NavPublication, PrintPolicy,
};
use payment_rails::{accounts as racc, instruction as rix, EngineGrant};
use solana_keypair::Keypair;
use solana_signer::Signer;

const SYSTEM: Pubkey = anchor_lang::system_program::ID;

/// Load `<name>.so` from the root workspace's `target/deploy`.
pub fn load_root_program(env: &mut Env, id: Pubkey, name: &str) {
    let so = format!(
        "{}/../../target/deploy/{name}.so",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&so)
        .unwrap_or_else(|e| panic!("{so}: {e} — run `anchor build` at the repo root"));
    env.svm.add_program(id, &bytes).expect("load engine");
}

/// One NPCS = 1 USDC at the NAV scale (micro-USDC per base unit × 10¹²).
pub const NAV_ONE: u128 = 1_000_000_000_000;
pub const WEEK: i64 = 7 * 86_400;

pub fn feed_params() -> FeedParams {
    FeedParams {
        warn_after: 8 * 86_400,
        stale_after: 15 * 86_400,
        warned_haircut_bps: 200,
        stale_haircut_bps: 1_000,
        print_policy: PrintPolicy::Public,
        band_bps: 100,
        min_print_volume: 1_000_000,
        min_print_investors: 2,
        outlier_band_bps: 2_000,
        drills_enabled: true,
    }
}

pub fn feed_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[Feed::SEED, mint.as_ref()], &mark_engine::ID).0
}

impl Env {
    pub fn load_mark_engine(&mut self) {
        load_root_program(self, mark_engine::ID, "mark_engine");
    }

    pub fn feed(&self, mint: &Pubkey) -> Feed {
        let a = self.svm.get_account(&feed_pda(mint)).expect("feed");
        Feed::try_deserialize(&mut &a.data[..]).unwrap()
    }

    /// Create the feed for `mint` (the rails authority is the feed
    /// authority) with `publisher` as its only publisher.
    pub fn init_feed(&mut self, mint: &Pubkey, params: FeedParams, publisher: &Pubkey) {
        let auth = self.authority.insecure_clone();
        let init = Instruction {
            program_id: mark_engine::ID,
            accounts: macc::InitializeFeed {
                feed: feed_pda(mint),
                asset_mint: *mint,
                authority: auth.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: mix::InitializeFeed { params }.data(),
        };
        let set = self.feed_admin_ix(
            mint,
            mix::SetPublishers {
                publishers: vec![*publisher],
            }
            .data(),
        );
        self.send(&[init, set], &[&auth]).expect("init feed");
    }

    pub fn feed_admin_ix(&self, mint: &Pubkey, data: Vec<u8>) -> Instruction {
        Instruction {
            program_id: mark_engine::ID,
            accounts: macc::FeedAdmin {
                feed: feed_pda(mint),
                authority: self.authority.pubkey(),
            }
            .to_account_metas(None),
            data,
        }
    }

    pub fn feed_admin(&mut self, mint: &Pubkey, data: Vec<u8>) -> Res {
        let ix = self.feed_admin_ix(mint, data);
        let a = self.authority.insecure_clone();
        self.send(&[ix], &[&a])
    }

    /// `publisher` publishes `nav` computed now.
    pub fn publish_nav(&mut self, mint: &Pubkey, publisher: &Keypair, nav: u128) -> Res {
        let now = self.now();
        let ix = Instruction {
            program_id: mark_engine::ID,
            accounts: macc::PublishNav {
                feed: feed_pda(mint),
                publisher: publisher.pubkey(),
            }
            .to_account_metas(None),
            data: mix::PublishNav {
                publication: NavPublication {
                    nav_pico: nav,
                    computed_ts: now,
                    model_version: 1,
                    inputs_hash: [7; 32],
                },
            }
            .data(),
        };
        self.send(&[ix], &[publisher])
    }

    /// AC-9 drill mark (feed authority; `drills_enabled`).
    pub fn drill_nav(&mut self, mint: &Pubkey, nav: u128) -> Res {
        let now = self.now();
        self.feed_admin(
            mint,
            mix::PublishDrillNav {
                publication: NavPublication {
                    nav_pico: nav,
                    computed_ts: now,
                    model_version: 1,
                    inputs_hash: [0xd; 32],
                },
            }
            .data(),
        )
    }

    pub fn refresh_feed(&mut self, mint: &Pubkey, caller: &Keypair) -> Res {
        let ix = Instruction {
            program_id: mark_engine::ID,
            accounts: macc::Refresh {
                feed: feed_pda(mint),
            }
            .to_account_metas(None),
            data: mix::Refresh {}.data(),
        };
        self.send(&[ix], &[caller])
    }
}

// ── auction-engine (#136) ───────────────────────────────────────────────

pub fn venue_pda() -> Pubkey {
    Pubkey::find_program_address(&[VENUE_SEED], &auction_engine::ID).0
}

pub fn auction_pda(id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            auction_engine::SESSION_SEED,
            venue_pda().as_ref(),
            &id.to_le_bytes(),
        ],
        &auction_engine::ID,
    )
    .0
}

pub fn grant_pda(engine: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[EngineGrant::SEED, engine.as_ref()], &payment_rails::ID).0
}

/// `metas` as remaining accounts of an engine instruction: the engine's
/// PDAs cannot sign the outer transaction, the engine signs for them.
pub fn forwarded(metas: Vec<AccountMeta>) -> Vec<AccountMeta> {
    let pdas = [operator_pda().0, print_pda().0];
    metas
        .into_iter()
        .map(|m| AccountMeta {
            is_signer: m.is_signer && !pdas.contains(&m.pubkey),
            ..m
        })
        .collect()
}

/// A sealed order before reveal.
#[derive(Clone, Copy)]
pub struct Sealed {
    pub side: Side,
    pub limit: u64,
    pub qty: u64,
    pub nonce: [u8; 32],
}

impl Env {
    /// mark-engine + auction-engine loaded; feed for the asset with a Fresh
    /// NAV of 1 USDC; venue opened; the operator PDA granted sessions on
    /// the rails; the print PDA set as the feed's print source.
    pub fn setup_auction(&mut self) -> Keypair {
        self.load_mark_engine();
        load_root_program(self, auction_engine::ID, "auction_engine");
        let mint = self.asset_mint;
        let publisher = self.funded();
        self.init_feed(&mint, feed_params(), &publisher.pubkey());
        self.publish_nav(&mint, &publisher, NAV_ONE).expect("nav");
        let auth = self.authority.insecure_clone();
        let venue = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::InitializeVenue {
                venue: venue_pda(),
                operator: operator_pda().0,
                authority: auth.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: aix::InitializeVenue {
                forced_sale_opener: Pubkey::default(),
            }
            .data(),
        };
        self.send(&[venue], &[&auth]).expect("venue");
        self.grant_engine(&operator_pda().0, EngineGrant::ROLE_SESSIONS)
            .expect("grant");
        self.feed_admin(
            &mint,
            mark_engine::instruction::SetPrintSource {
                print_source: print_pda().0,
            }
            .data(),
        )
        .expect("print source");
        publisher
    }

    pub fn grant_engine(&mut self, engine: &Pubkey, roles: u8) -> Res {
        let auth = self.authority.insecure_clone();
        let ix = Instruction {
            program_id: payment_rails::ID,
            accounts: racc::GrantEngine {
                config: self.config,
                grant: grant_pda(engine),
                authority: auth.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: rix::GrantEngine {
                engine: *engine,
                roles,
            }
            .data(),
        };
        self.send(&[ix], &[&auth])
    }

    pub fn auction(&self, id: u64) -> AuctionSession {
        let a = self.svm.get_account(&auction_pda(id)).expect("session");
        AuctionSession::try_deserialize(&mut &a.data[..]).unwrap()
    }

    /// The rails `CashSession` behind auction `id`.
    pub fn rails_session_of(&self, id: u64) -> Pubkey {
        session_pda(&operator_pda().0, id)
    }

    pub fn open_auction_as(&mut self, opener: &Keypair, id: u64, params: AuctionParams) -> Res {
        let operator = operator_pda().0;
        let mut ix = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::OpenSession {
                venue: venue_pda(),
                session: auction_pda(id),
                feed: feed_pda(&self.asset_mint),
                operator,
                opener: opener.pubkey(),
                rails_program: payment_rails::ID,
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: aix::OpenSession {
                session_id: id,
                params,
            }
            .data(),
        };
        let mut rails = racc::OpenSession {
            config: self.config,
            session: self.rails_session_of(id),
            asset_mint: self.asset_mint,
            operator,
            system_program: SYSTEM,
        }
        .to_account_metas(None);
        rails.push(AccountMeta::new_readonly(grant_pda(&operator), false));
        ix.accounts.extend(forwarded(rails));
        self.send(&[ix], &[opener])
    }

    pub fn open_auction(&mut self, id: u64, params: AuctionParams) -> Res {
        let a = self.authority.insecure_clone();
        self.open_auction_as(&a, id, params)
    }

    pub fn commit_bid(&mut self, id: u64, who: &Party, o: &Sealed, cash: u64) -> Res {
        let rs = self.rails_session_of(id);
        let escrow = escrow_pda(&rs, &who.key.pubkey());
        let owner = who.key.pubkey();
        let rails = racc::Deposit {
            config: self.config,
            session: rs,
            escrow,
            vault: vault_pda(&escrow),
            usdc_mint: self.mint,
            owner_token: who.usdc,
            owner,
            token_program: self.tp.id(),
            system_program: SYSTEM,
        }
        .to_account_metas(None);
        let c = auction_engine::commitment(o.side, o.limit, o.qty, &o.nonce, &owner);
        self.commit_ix(
            id,
            who,
            rails,
            aix::CommitBid {
                commitment_hash: c,
                cash,
            }
            .data(),
        )
    }

    /// Open the seller's rails asset vault and commit the ask, one tx.
    pub fn commit_ask(&mut self, id: u64, who: &Party, o: &Sealed, escrow_qty: u64) -> Res {
        let rs = self.rails_session_of(id);
        let owner = who.key.pubkey();
        let escrow = escrow_pda(&rs, &owner);
        let rails = racc::DepositAsk {
            config: self.config,
            session: rs,
            escrow,
            vault: vault_pda(&escrow),
            asset_vault: asset_vault_pda(&escrow),
            usdc_mint: self.mint,
            asset_mint: self.asset_mint,
            owner_usdc: who.usdc,
            owner_asset: who.asset,
            owner,
            token_program: self.tp.id(),
            asset_token_program: self.asset_tp.id(),
            system_program: SYSTEM,
        }
        .to_account_metas(None);
        let open = self.open_asset_vault_ix(&rs, &owner);
        let c = auction_engine::commitment(o.side, o.limit, o.qty, &o.nonce, &owner);
        let ix = self.commit_raw_ix(
            id,
            who,
            rails,
            aix::CommitAsk {
                commitment_hash: c,
                qty: escrow_qty,
            }
            .data(),
        );
        self.send(&[open, ix], &[&who.key])
    }

    fn commit_raw_ix(
        &self,
        id: u64,
        who: &Party,
        rails: Vec<AccountMeta>,
        data: Vec<u8>,
    ) -> Instruction {
        let mut ix = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::Commit {
                session: auction_pda(id),
                owner: who.key.pubkey(),
                rails_program: payment_rails::ID,
            }
            .to_account_metas(None),
            data,
        };
        ix.accounts.extend(rails);
        ix
    }

    fn commit_ix(&mut self, id: u64, who: &Party, rails: Vec<AccountMeta>, data: Vec<u8>) -> Res {
        let ix = self.commit_raw_ix(id, who, rails, data);
        self.send(&[ix], &[&who.key])
    }

    pub fn reveal(&mut self, id: u64, who: &Party, o: &Sealed) -> Res {
        let rs = self.rails_session_of(id);
        let operator = operator_pda().0;
        let mut ix = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::Reveal {
                venue: venue_pda(),
                session: auction_pda(id),
                owner: who.key.pubkey(),
                operator,
                rails_program: payment_rails::ID,
            }
            .to_account_metas(None),
            data: aix::Reveal {
                limit: o.limit,
                qty: o.qty,
                nonce: o.nonce,
            }
            .data(),
        };
        ix.accounts.extend(forwarded(
            racc::OperatorEscrow {
                session: rs,
                escrow: escrow_pda(&rs, &who.key.pubkey()),
                operator,
            }
            .to_account_metas(None),
        ));
        self.send(&[ix], &[&who.key])
    }

    fn engine_session_ix(&self, id: u64, accounts: Vec<AccountMeta>, data: Vec<u8>) -> Instruction {
        let mut ix = Instruction {
            program_id: auction_engine::ID,
            accounts,
            data,
        };
        ix.accounts.extend(forwarded(
            racc::OperatorSession {
                session: self.rails_session_of(id),
                operator: operator_pda().0,
            }
            .to_account_metas(None),
        ));
        ix
    }

    fn clear_accounts(&self, id: u64) -> Vec<AccountMeta> {
        aacc::Clear {
            venue: venue_pda(),
            session: auction_pda(id),
            feed: feed_pda(&self.asset_mint),
            operator: operator_pda().0,
            rails_program: payment_rails::ID,
        }
        .to_account_metas(None)
    }

    pub fn clear_auction(&mut self, id: u64, caller: &Keypair) -> Res {
        let ix = self.engine_session_ix(id, self.clear_accounts(id), aix::Clear {}.data());
        self.send(&[ix], &[caller])
    }

    pub fn expire_auction(&mut self, id: u64, caller: &Keypair) -> Res {
        let ix = self.engine_session_ix(id, self.clear_accounts(id), aix::Expire {}.data());
        self.send(&[ix], &[caller])
    }

    pub fn allocate(&mut self, id: u64, caller: &Keypair, max_fills: u8) -> Res {
        let ix = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::Crank {
                session: auction_pda(id),
            }
            .to_account_metas(None),
            data: aix::Allocate { max_fills }.data(),
        };
        self.send(&[ix], &[caller])
    }

    pub fn exercise_rofr(
        &mut self,
        id: u64,
        issuer: &Party,
        seller: &Party,
        fill_index: u32,
        qty: u64,
    ) -> Res {
        let rs = self.rails_session_of(id);
        let operator = operator_pda().0;
        let new_index = self.auction(id).fills.len() as u32;
        let seller_escrow = escrow_pda(&rs, &seller.key.pubkey());
        let mut ix = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::ExerciseRofr {
                venue: venue_pda(),
                session: auction_pda(id),
                issuer: issuer.key.pubkey(),
                operator,
                rails_program: payment_rails::ID,
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: aix::ExerciseRofr { fill_index, qty }.data(),
        };
        ix.accounts.extend(forwarded(
            racc::SettleRofrFill {
                config: self.config,
                session: rs,
                operator,
                issuer: issuer.key.pubkey(),
                receipt: receipt_pda(&rs, new_index),
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
                system_program: SYSTEM,
                gate: racc::RofrGate {
                    gate_program: super::GATE_ID,
                    gate_authority: super::gate_settle_pda(),
                    gate_config: self.gate_config(),
                    seller_block: self.block_entry(&seller.key.pubkey()),
                    issuer_block: self.block_entry(&issuer.key.pubkey()),
                    seller_lots: self.lots_pda(&seller.key.pubkey()),
                },
            }
            .to_account_metas(None),
        ));
        self.send(&[ix], &[&issuer.key])
    }

    pub fn close_rofr(&mut self, id: u64, caller: &Keypair) -> Res {
        let ix = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::CloseRofr {
                session: auction_pda(id),
                caller: caller.pubkey(),
            }
            .to_account_metas(None),
            data: aix::CloseRofr {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    /// Settle auction fill `fill_index` between `buyer` and `seller`.
    pub fn settle_auction(
        &mut self,
        id: u64,
        caller: &Keypair,
        fill_index: u32,
        buyer: &Party,
        seller: &Party,
    ) -> Res {
        let rs = self.rails_session_of(id);
        let operator = operator_pda().0;
        let f: Fill = self.fill(&rs, buyer, seller);
        let rails = self.settle_ix(&operator, &f, fill_index, 0, 0);
        let mut ix = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::Settle {
                venue: venue_pda(),
                session: auction_pda(id),
                operator,
                payer: caller.pubkey(),
                rails_program: payment_rails::ID,
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: aix::Settle { fill_index }.data(),
        };
        ix.accounts.extend(forwarded(rails.accounts));
        self.send(&[ix], &[caller])
    }

    pub fn finish_auction(&mut self, id: u64, caller: &Keypair) -> Res {
        let feed = feed_pda(&self.asset_mint);
        let session = auction_pda(id);
        let accounts = aacc::Finish {
            venue: venue_pda(),
            session,
            feed,
            print_record: Pubkey::find_program_address(
                &[
                    mark_engine::PrintRecord::SEED,
                    feed.as_ref(),
                    session.as_ref(),
                ],
                &mark_engine::ID,
            )
            .0,
            print_authority: print_pda().0,
            operator: operator_pda().0,
            payer: caller.pubkey(),
            rails_program: payment_rails::ID,
            mark_program: mark_engine::ID,
            system_program: SYSTEM,
        }
        .to_account_metas(None);
        let ix = self.engine_session_ix(id, accounts, aix::Finish {}.data());
        self.send(&[ix], &[caller])
    }
}
