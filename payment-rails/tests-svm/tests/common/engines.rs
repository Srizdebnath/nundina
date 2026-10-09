//! The engine programs (#136–#140) next to the rails and the gate: loaders
//! for their root-workspace `anchor build` artifacts and instruction
//! wrappers shared by the engine suites.

use super::gate::{gate_pda, investor_of, GATE_ID};
use super::{
    advance_pda, asset_vault_pda, dest_pda, escrow_pda, facility_pda, facility_vault_pda,
    receipt_pda, session_pda, vault_pda, Env, Fill, Party, Res,
};
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use auction_engine::{
    accounts as aacc, instruction as aix, operator_pda, print_pda, Session as AuctionSession,
    SessionParams as AuctionParams, Side, VENUE_SEED,
};
use credit_gate::{
    accounts as gacc, instruction as gix, pledge_holder, Custodian, HolderLink, InvestorRecord,
    MarketConfig,
};
use liquidity_facility::{
    accounts as lacc, instruction as lix, signer_pda as lf_signer, FacilityParams, ForcedSchedule,
    Position,
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
    // As a real deploy: the rails authority is each engine's upgrade
    // authority, which gates the init / open instructions (first caller
    // would otherwise own them). ProgramData: u32 tag | u64 slot | Option<Pubkey>.
    let pd_addr = programdata(&id);
    let mut pd = env.svm.get_account(&pd_addr).expect("engine programdata");
    pd.data[12] = 1;
    pd.data[13..45].copy_from_slice(env.authority.pubkey().as_ref());
    env.svm.set_account(pd_addr, pd).unwrap();
}

/// An upgradeable program's ProgramData address.
pub fn programdata(id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[id.as_ref()], &super::LOADER_V3).0
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
    /// `initialize_feed` signed by `authority` (must be mark-engine's upgrade authority).
    pub fn init_feed_ix(
        &self,
        mint: &Pubkey,
        params: FeedParams,
        authority: &Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: mark_engine::ID,
            accounts: macc::InitializeFeed {
                feed: feed_pda(mint),
                asset_mint: *mint,
                authority: *authority,
                program: mark_engine::ID,
                program_data: programdata(&mark_engine::ID),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: mix::InitializeFeed { params }.data(),
        }
    }

    pub fn init_feed(&mut self, mint: &Pubkey, params: FeedParams, publisher: &Pubkey) {
        let auth = self.authority.insecure_clone();
        let init = self.init_feed_ix(mint, params, &auth.pubkey());
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

/// `initialize_venue` signed by `authority` (must be auction-engine's upgrade authority).
pub fn venue_init_ix(authority: &Pubkey) -> Instruction {
    Instruction {
        program_id: auction_engine::ID,
        accounts: aacc::InitializeVenue {
            venue: venue_pda(),
            operator: operator_pda().0,
            authority: *authority,
            program: auction_engine::ID,
            program_data: programdata(&auction_engine::ID),
            system_program: SYSTEM,
        }
        .to_account_metas(None),
        data: aix::InitializeVenue {
            forced_sale_opener: Pubkey::default(),
        }
        .data(),
    }
}

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
        let venue = venue_init_ix(&auth.pubkey());
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

    /// AE-P6 screen of order `index` (bid or ask, by its side).
    pub fn screen_ix(&self, id: u64, index: u8) -> Instruction {
        let session = self.auction(id);
        let order = session.orders[usize::from(index)];
        let owner = order.owner;
        let mint = self.asset_mint;
        match order.side {
            Side::Buy => {
                let link = self.svm.get_account(&self.link_pda(&owner));
                let investor_id = link
                    .and_then(|a| HolderLink::try_deserialize(&mut &a.data[..]).ok())
                    .map(|l| l.investor_id)
                    .unwrap_or(investor_of(&owner));
                Instruction {
                    program_id: auction_engine::ID,
                    accounts: aacc::ScreenBid {
                        session: auction_pda(id),
                        gate_config: self.gate_config(),
                        gate_policy: self.gate_policy(),
                        block_entry: self.block_entry(&owner),
                        holder_link: self.link_pda(&owner),
                        investor: gate_pda(&[InvestorRecord::SEED, mint.as_ref(), &investor_id]),
                        buyer_lots: self.lots_pda(&owner),
                        identity_attestation: self.identity_att(&owner),
                        accreditation_attestation: self.accreditation_att(&owner),
                        mint,
                        gate_program: credit_gate::ID,
                    }
                    .to_account_metas(None),
                    data: aix::ScreenBid { order_index: index }.data(),
                }
            }
            Side::Sell => Instruction {
                program_id: auction_engine::ID,
                accounts: aacc::ScreenAsk {
                    session: auction_pda(id),
                    gate_config: self.gate_config(),
                    block_entry: self.block_entry(&owner),
                    seller_lots: self.lots_pda(&owner),
                }
                .to_account_metas(None),
                data: aix::ScreenAsk { order_index: index }.data(),
            },
        }
    }

    pub fn screen(&mut self, id: u64, index: u8, caller: &Keypair) -> Res {
        let ix = self.screen_ix(id, index);
        self.send(&[ix], &[caller])
    }

    /// AE-P6: screen every revealed, unscreened order (what the crank does).
    pub fn screen_all(&mut self, id: u64, caller: &Keypair) {
        let orders = self.auction(id).orders;
        for (i, o) in orders.iter().enumerate() {
            if o.revealed && !o.screened {
                self.screen(id, i as u8, caller).expect("screen");
            }
        }
    }

    /// `clear` alone, without screening first.
    pub fn clear_only(&mut self, id: u64, caller: &Keypair) -> Res {
        let ix = self.engine_session_ix(id, self.clear_accounts(id), aix::Clear {}.data());
        self.send(&[ix], &[caller])
    }

    /// Screen every revealed order (AE-P6), then `clear`. Screening needs
    /// `reveal_end` to have passed; before that, `clear` runs bare so its
    /// own window check is what fails.
    pub fn clear_auction(&mut self, id: u64, caller: &Keypair) -> Res {
        let s = self.auction(id);
        if self.now() >= s.params.reveal_end && self.now() < s.params.settle_deadline {
            self.screen_all(id, caller);
        }
        self.clear_only(id, caller)
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
        let f: Fill = self.fill(&rs, buyer, seller);
        self.settle_auction_fill(id, caller, fill_index, f)
    }

    /// Settle auction fill `fill_index` with an explicit rails `Fill`
    /// (a PDA seller, e.g. the facility's signer).
    pub fn settle_auction_fill(
        &mut self,
        id: u64,
        caller: &Keypair,
        fill_index: u32,
        f: Fill,
    ) -> Res {
        let operator = operator_pda().0;
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

// ── liquidity-facility (#139) ───────────────────────────────────────────

pub fn lf_config_pda(id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[liquidity_facility::FACILITY_SEED, &id.to_le_bytes()],
        &liquidity_facility::ID,
    )
    .0
}

pub fn position_pda(cfg: &Pubkey, borrower: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            liquidity_facility::POSITION_SEED,
            cfg.as_ref(),
            borrower.as_ref(),
        ],
        &liquidity_facility::ID,
    )
    .0
}

pub fn collateral_pda(position: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[liquidity_facility::COLLATERAL_SEED, position.as_ref()],
        &liquidity_facility::ID,
    )
    .0
}

pub fn proceeds_pda(position: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[liquidity_facility::PROCEEDS_SEED, position.as_ref()],
        &liquidity_facility::ID,
    )
    .0
}

/// Rails facility of liquidity-facility `id` (curator = its signer PDA).
pub fn lf_rails_facility(id: u64) -> Pubkey {
    facility_pda(&lf_signer().0, id)
}

impl Env {
    /// The facility signer's custodian registration (LF-P5).
    pub fn custodian_record(&self) -> Pubkey {
        gate_pda(&[
            Custodian::SEED,
            self.asset_mint.as_ref(),
            lf_signer().0.as_ref(),
        ])
    }

    /// The pledge ledger of `position` (LF-P5).
    pub fn pledge_lots(&self, position: &Pubkey) -> Pubkey {
        self.lots_pda(&pledge_holder(&self.asset_mint, &lf_signer().0, position))
    }

    /// The gate accounts of a custody move: `position`'s pledge ledger and
    /// `other`'s ledger and block entry.
    pub fn lf_gate(&self, position: &Pubkey, other: &Pubkey) -> lacc::GateCustody {
        lacc::GateCustody {
            gate_program: GATE_ID,
            gate_config: gate_pda(&[MarketConfig::SEED, self.asset_mint.as_ref()]),
            custodian_record: self.custodian_record(),
            pledge_lots: self.pledge_lots(position),
            other_lots: self.lots_pda(other),
            block_entry: self.block_entry(other),
        }
    }

    /// `register_custodian(signer PDA)` by the gate authority (LF-P5).
    pub fn register_lf_custodian(&mut self) -> Res {
        let auth = self.authority.insecure_clone();
        let ix = Instruction {
            program_id: GATE_ID,
            accounts: gacc::RegisterCustodian {
                config: self.gate_config(),
                record: self.custodian_record(),
                mint: self.asset_mint,
                gate_config_authority: auth.pubkey(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: gix::RegisterCustodian {
                custodian: lf_signer().0,
            }
            .data(),
        };
        self.send(&[ix], &[&auth])
    }

    pub fn position(&self, cfg: &Pubkey, borrower: &Pubkey) -> Position {
        let a = self
            .svm
            .get_account(&position_pda(cfg, borrower))
            .expect("position");
        Position::try_deserialize(&mut &a.data[..]).unwrap()
    }

    /// Auction setup + the facility signer granted FACILITY on the rails,
    /// made the venue's forced-sale opener, and facility `id` opened.
    pub fn setup_facility(&mut self, id: u64, params: FacilityParams) -> Keypair {
        let publisher = self.setup_auction();
        load_root_program(self, liquidity_facility::ID, "liquidity_facility");
        let signer = lf_signer().0;
        self.grant_engine(&signer, EngineGrant::ROLE_FACILITY)
            .expect("grant facility");
        let auth = self.authority.insecure_clone();
        let opener = Instruction {
            program_id: auction_engine::ID,
            accounts: aacc::VenueAdmin {
                venue: venue_pda(),
                authority: auth.pubkey(),
            }
            .to_account_metas(None),
            data: aix::SetForcedSaleOpener { opener: signer }.data(),
        };
        self.send(&[opener], &[&auth]).expect("forced-sale opener");
        let ix = self.open_facility_ix(id, params, &auth.pubkey());
        self.send(&[ix], &[&auth]).expect("open facility");
        // LF-P5: on a gated mint the signer is the market's custodian (a
        // queue-claim token has no gate market).
        if self.exists(&self.gate_config()) {
            self.register_lf_custodian().expect("custodian");
        }
        publisher
    }

    /// `open_facility` signed by `authority` (must be liquidity-facility's upgrade authority).
    pub fn open_facility_ix(
        &self,
        id: u64,
        params: FacilityParams,
        authority: &Pubkey,
    ) -> Instruction {
        let signer = lf_signer().0;
        let rails_facility = lf_rails_facility(id);
        let mut ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::OpenFacility {
                config: lf_config_pda(id),
                signer,
                feed: feed_pda(&self.asset_mint),
                usdc_mint: self.mint,
                authority: *authority,
                program: liquidity_facility::ID,
                program_data: programdata(&liquidity_facility::ID),
                rails_program: payment_rails::ID,
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: lix::OpenFacility {
                facility_id: id,
                params,
            }
            .data(),
        };
        let mut rails = racc::OpenFacility {
            config: self.config,
            facility: rails_facility,
            vault: facility_vault_pda(&rails_facility),
            usdc_mint: self.mint,
            operator: signer,
            token_program: self.tp.id(),
            system_program: SYSTEM,
        }
        .to_account_metas(None);
        rails.push(AccountMeta::new_readonly(grant_pda(&signer), false));
        ix.accounts.extend(forwarded_lf(rails));
        ix
    }

    pub fn pledge(&mut self, id: u64, borrower: &Party, qty: u64, amount: u64) -> Res {
        let ix = self.pledge_ix(id, borrower, qty, amount);
        self.send(&[ix], &[&borrower.key])
    }

    pub fn pledge_ix(&self, id: u64, borrower: &Party, qty: u64, amount: u64) -> Instruction {
        let cfg = lf_config_pda(id);
        let b = borrower.key.pubkey();
        let position = position_pda(&cfg, &b);
        let signer = lf_signer().0;
        let rails_facility = lf_rails_facility(id);
        let mut ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::PledgeAndAdvance {
                config: cfg,
                position,
                collateral_vault: collateral_pda(&position),
                proceeds: proceeds_pda(&position),
                feed: feed_pda(&self.asset_mint),
                signer,
                borrower: b,
                borrower_asset: borrower.asset,
                asset_mint: self.asset_mint,
                usdc_mint: self.mint,
                asset_token_program: self.asset_tp.id(),
                token_program: self.tp.id(),
                rails_program: payment_rails::ID,
                system_program: SYSTEM,
                gate: lacc::GateCustody {
                    // `block_entry` is the pledge holder's: the ledger it opens.
                    block_entry: self.block_entry(&pledge_holder(
                        &self.asset_mint,
                        &signer,
                        &position,
                    )),
                    ..self.lf_gate(&position, &b)
                },
            }
            .to_account_metas(None),
            data: lix::PledgeAndAdvance { qty, amount }.data(),
        };
        ix.accounts.extend(forwarded_lf(
            racc::AdvanceFunds {
                config: self.config,
                facility: rails_facility,
                advance: advance_pda(&rails_facility, &b),
                vault: facility_vault_pda(&rails_facility),
                borrower: b,
                destination: dest_pda(&b),
                destination_token: borrower.usdc,
                usdc_mint: self.mint,
                curator: signer,
                token_program: self.tp.id(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
        ));
        ix
    }

    fn health_ix(&self, id: u64, borrower: &Pubkey, data: Vec<u8>) -> Instruction {
        let cfg = lf_config_pda(id);
        let rails_facility = lf_rails_facility(id);
        Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::CheckHealth {
                config: cfg,
                position: position_pda(&cfg, borrower),
                feed: feed_pda(&self.asset_mint),
                rails_facility,
                advance: advance_pda(&rails_facility, borrower),
            }
            .to_account_metas(None),
            data,
        }
    }

    pub fn check_health(&mut self, id: u64, borrower: &Pubkey, caller: &Keypair) -> Res {
        let ix = self.health_ix(id, borrower, lix::CheckHealth {}.data());
        self.send(&[ix], &[caller])
    }

    pub fn start_liquidation(&mut self, id: u64, borrower: &Pubkey, caller: &Keypair) -> Res {
        let ix = self.health_ix(id, borrower, lix::StartLiquidation {}.data());
        self.send(&[ix], &[caller])
    }

    fn lf_repay_metas(&self, id: u64, borrower: &Pubkey) -> Vec<AccountMeta> {
        let rails_facility = lf_rails_facility(id);
        let position = position_pda(&lf_config_pda(id), borrower);
        forwarded_lf(
            racc::Repay {
                facility: rails_facility,
                advance: advance_pda(&rails_facility, borrower),
                vault: facility_vault_pda(&rails_facility),
                curator: lf_signer().0,
                payer_token: proceeds_pda(&position),
                payer: lf_signer().0,
                usdc_mint: self.mint,
                token_program: self.tp.id(),
            }
            .to_account_metas(None),
        )
    }

    pub fn ladder_buy(&mut self, id: u64, borrower: &Party, buyer: &Party) -> Res {
        let cfg = lf_config_pda(id);
        let b = borrower.key.pubkey();
        let position = position_pda(&cfg, &b);
        let mut ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::LadderBuy {
                config: cfg,
                position,
                feed: feed_pda(&self.asset_mint),
                signer: lf_signer().0,
                buyer: buyer.key.pubkey(),
                buyer_usdc: buyer.usdc,
                buyer_asset: buyer.asset,
                collateral_vault: collateral_pda(&position),
                proceeds: proceeds_pda(&position),
                borrower_usdc: borrower.usdc,
                asset_mint: self.asset_mint,
                usdc_mint: self.mint,
                token_program: self.tp.id(),
                asset_token_program: self.asset_tp.id(),
                rails_program: payment_rails::ID,
                buyer_link: self.link_pda(&buyer.key.pubkey()),
                gate: self.lf_gate(&position, &buyer.key.pubkey()),
            }
            .to_account_metas(None),
            data: lix::LadderBuy {}.data(),
        };
        ix.accounts.extend(self.lf_repay_metas(id, &b));
        self.send(&[ix], &[&buyer.key])
    }

    pub fn pass_step(&mut self, id: u64, borrower: &Pubkey, caller: &Keypair) -> Res {
        let cfg = lf_config_pda(id);
        let ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::PassStep {
                config: cfg,
                position: position_pda(&cfg, borrower),
                caller: caller.pubkey(),
            }
            .to_account_metas(None),
            data: lix::PassStep {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    pub fn open_forced_sale(
        &mut self,
        id: u64,
        borrower: &Pubkey,
        caller: &Keypair,
        session_id: u64,
        schedule: ForcedSchedule,
    ) -> Res {
        let cfg = lf_config_pda(id);
        let signer = lf_signer().0;
        let operator = operator_pda().0;
        let mut ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::ForcedSale {
                config: cfg,
                position: position_pda(&cfg, borrower),
                feed: feed_pda(&self.asset_mint),
                signer,
                caller: caller.pubkey(),
                auction_program: auction_engine::ID,
                system_program: SYSTEM,
            }
            .to_account_metas(None),
            data: lix::OpenForcedSale {
                session_id,
                schedule,
            }
            .data(),
        };
        let mut inner = aacc::OpenSession {
            venue: venue_pda(),
            session: auction_pda(session_id),
            feed: feed_pda(&self.asset_mint),
            operator,
            opener: signer,
            rails_program: payment_rails::ID,
            system_program: SYSTEM,
        }
        .to_account_metas(None);
        inner.extend(
            racc::OpenSession {
                config: self.config,
                session: self.rails_session_of(session_id),
                asset_mint: self.asset_mint,
                operator,
                system_program: SYSTEM,
            }
            .to_account_metas(None),
        );
        inner.push(AccountMeta::new_readonly(grant_pda(&operator), false));
        ix.accounts.extend(forwarded_lf(inner));
        self.send(&[ix], &[caller])
    }

    pub fn list_forced_sale(
        &mut self,
        id: u64,
        borrower: &Pubkey,
        caller: &Party,
        session_id: u64,
    ) -> Res {
        let cfg = lf_config_pda(id);
        let position = position_pda(&cfg, borrower);
        let signer = lf_signer().0;
        let rs = self.rails_session_of(session_id);
        let escrow = escrow_pda(&rs, &signer);
        let mut ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::ListForcedSale {
                config: cfg,
                position,
                signer,
                caller: caller.key.pubkey(),
                caller_usdc: caller.usdc,
                proceeds: proceeds_pda(&position),
                usdc_mint: self.mint,
                token_program: self.tp.id(),
                rails_program: payment_rails::ID,
                auction_program: auction_engine::ID,
                asset_mint: self.asset_mint,
                system_program: SYSTEM,
                gate: self.lf_gate(&position, &signer),
            }
            .to_account_metas(None),
            data: lix::ListForcedSale {}.data(),
        };
        let mut inner = self.open_asset_vault_ix(&rs, &signer).accounts;
        inner.extend(
            aacc::Commit {
                session: auction_pda(session_id),
                owner: signer,
                rails_program: payment_rails::ID,
            }
            .to_account_metas(None),
        );
        inner.extend(
            racc::DepositAsk {
                config: self.config,
                session: rs,
                escrow,
                vault: vault_pda(&escrow),
                asset_vault: asset_vault_pda(&escrow),
                usdc_mint: self.mint,
                asset_mint: self.asset_mint,
                owner_usdc: proceeds_pda(&position),
                owner_asset: collateral_pda(&position),
                owner: signer,
                token_program: self.tp.id(),
                asset_token_program: self.asset_tp.id(),
                system_program: SYSTEM,
            }
            .to_account_metas(None),
        );
        ix.accounts.extend(forwarded_lf(inner));
        self.send(&[ix], &[&caller.key])
    }

    pub fn reveal_forced_sale(
        &mut self,
        id: u64,
        borrower: &Pubkey,
        caller: &Keypair,
        session_id: u64,
    ) -> Res {
        let cfg = lf_config_pda(id);
        let signer = lf_signer().0;
        let operator = operator_pda().0;
        let rs = self.rails_session_of(session_id);
        let mut ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::RevealForcedSale {
                config: cfg,
                position: position_pda(&cfg, borrower),
                signer,
                auction_program: auction_engine::ID,
            }
            .to_account_metas(None),
            data: lix::RevealForcedSale {}.data(),
        };
        let mut inner = aacc::Reveal {
            venue: venue_pda(),
            session: auction_pda(session_id),
            owner: signer,
            operator,
            rails_program: payment_rails::ID,
        }
        .to_account_metas(None);
        inner.extend(
            racc::OperatorEscrow {
                session: rs,
                escrow: escrow_pda(&rs, &signer),
                operator,
            }
            .to_account_metas(None),
        );
        ix.accounts.extend(forwarded_lf(inner));
        self.send(&[ix], &[caller])
    }

    /// The rails `Fill` of a forced-sale fill (seller = facility signer).
    pub fn forced_fill(&self, id: u64, borrower: &Pubkey, session_id: u64, buyer: &Party) -> Fill {
        let position = position_pda(&lf_config_pda(id), borrower);
        Fill {
            session: self.rails_session_of(session_id),
            buyer_owner: buyer.key.pubkey(),
            seller_owner: lf_signer().0,
            seller_usdc: proceeds_pda(&position),
            buyer_asset: buyer.asset,
            fee_receiver: self.fee,
            reserve: self.reserve,
        }
    }

    /// Rails `refund_ask` of the facility signer's forced-sale escrow.
    pub fn refund_forced_ask(
        &mut self,
        id: u64,
        borrower: &Pubkey,
        caller: &Keypair,
        session_id: u64,
    ) -> Res {
        let position = position_pda(&lf_config_pda(id), borrower);
        let signer = lf_signer().0;
        let rs = self.rails_session_of(session_id);
        let escrow = escrow_pda(&rs, &signer);
        let ix = Instruction {
            program_id: payment_rails::ID,
            accounts: racc::RefundAsk {
                caller: caller.pubkey(),
                session: rs,
                escrow,
                vault: vault_pda(&escrow),
                asset_vault: asset_vault_pda(&escrow),
                owner: signer,
                owner_usdc: proceeds_pda(&position),
                owner_asset: collateral_pda(&position),
                reserve: self.reserve,
                usdc_mint: self.mint,
                asset_mint: self.asset_mint,
                token_program: self.tp.id(),
                asset_token_program: self.asset_tp.id(),
            }
            .to_account_metas(None),
            data: rix::RefundAsk {}.data(),
        };
        self.send(&[ix], &[caller])
    }

    pub fn settle_forced_sale(
        &mut self,
        id: u64,
        borrower: &Party,
        caller: &Keypair,
        session_id: u64,
        workout_asset: &Pubkey,
    ) -> Res {
        let cfg = lf_config_pda(id);
        let b = borrower.key.pubkey();
        let position = position_pda(&cfg, &b);
        let signer = lf_signer().0;
        let rs = self.rails_session_of(session_id);
        let workout_owner = self.token_owner(workout_asset);
        let mut ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::SettleForcedSale {
                config: cfg,
                position,
                auction_session: auction_pda(session_id),
                ask_escrow: escrow_pda(&rs, &signer),
                signer,
                collateral_vault: collateral_pda(&position),
                proceeds: proceeds_pda(&position),
                borrower_usdc: borrower.usdc,
                workout_asset: *workout_asset,
                asset_mint: self.asset_mint,
                usdc_mint: self.mint,
                token_program: self.tp.id(),
                asset_token_program: self.asset_tp.id(),
                rails_program: payment_rails::ID,
                signer_lots: self.lots_pda(&signer),
                payer: caller.pubkey(),
                system_program: SYSTEM,
                gate: self.lf_gate(&position, &workout_owner),
            }
            .to_account_metas(None),
            data: lix::SettleForcedSale {}.data(),
        };
        ix.accounts.extend(self.lf_repay_metas(id, &b));
        self.send(&[ix], &[caller])
    }

    pub fn release(&mut self, id: u64, borrower: &Party, caller: &Keypair) -> Res {
        let cfg = lf_config_pda(id);
        let b = borrower.key.pubkey();
        let position = position_pda(&cfg, &b);
        let ix = Instruction {
            program_id: liquidity_facility::ID,
            accounts: lacc::Release {
                config: cfg,
                position,
                advance: advance_pda(&lf_rails_facility(id), &b),
                signer: lf_signer().0,
                collateral_vault: collateral_pda(&position),
                borrower_asset: borrower.asset,
                asset_mint: self.asset_mint,
                asset_token_program: self.asset_tp.id(),
                gate: self.lf_gate(&position, &b),
            }
            .to_account_metas(None),
            data: lix::Release {}.data(),
        };
        self.send(&[ix], &[caller])
    }
}

/// Like [`forwarded`], for the facility signer PDA too.
pub fn forwarded_lf(metas: Vec<AccountMeta>) -> Vec<AccountMeta> {
    let signer = lf_signer().0;
    forwarded(metas)
        .into_iter()
        .map(|m| AccountMeta {
            is_signer: m.is_signer && m.pubkey != signer,
            ..m
        })
        .collect()
}
