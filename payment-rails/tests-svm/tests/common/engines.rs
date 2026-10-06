//! The engine programs (#136–#140) next to the rails and the gate: loaders
//! for their root-workspace `anchor build` artifacts and instruction
//! wrappers shared by the engine suites.

use super::{Env, Res};
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use mark_engine::{
    accounts as macc, instruction as mix, Feed, FeedParams, NavPublication, PrintPolicy,
};
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
