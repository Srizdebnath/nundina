//! Stable-toolchain smoke run of every fuzz invariant (#125): a fixed-seed
//! byte stream through `arbitrary`, so CI checks the same laws as the
//! libFuzzer targets without nightly. Fixed seed: a failure reproduces.

use arbitrary::{Arbitrary, Unstructured};
use payment_rails_fuzz as f;

const CASES: usize = 20_000;

/// xorshift64*: deterministic, dependency-free byte source.
fn bytes(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed.max(1);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        out.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    out.truncate(n);
    out
}

/// Feed `CASES` inputs to `check`. Every 4th input draws its bytes from a
/// small alphabet {0, 1, 0xff, …} so edge values (0, 1, MAX) are common.
///
/// Fails if fewer than a third of the cases reach the Ok path: a harness
/// that only ever sees rejections checks no conservation law at all.
fn run<T: for<'a> Arbitrary<'a> + std::fmt::Debug>(name: &str, check: fn(T) -> bool) {
    let mut ok = 0usize;
    for case in 0..CASES {
        let mut data = bytes(0x9E37_79B9_7F4A_7C15 ^ case as u64, 512);
        if case % 4 == 0 {
            for b in &mut data {
                *b = [0, 1, 0xff, 0x7f, 0x80, 2][usize::from(*b) % 6];
            }
        }
        let mut u = Unstructured::new(&data);
        if let Ok(input) = T::arbitrary(&mut u) {
            ok += usize::from(check(input));
        }
    }
    eprintln!("{name}: {CASES} cases, {ok} on the Ok path");
    assert!(
        ok * 3 >= CASES,
        "{name}: only {ok}/{CASES} cases reached the Ok path"
    );
}

#[test]
fn settle_fill() {
    run("settle_fill", f::check_settle_fill);
}

#[test]
fn clearing() {
    run("clearing", f::check_clearing);
}

#[test]
fn pro_rata() {
    run("pro_rata", f::check_pro_rata);
}

#[test]
fn waterfall() {
    run("waterfall", f::check_waterfall);
}

#[test]
fn refund_bond() {
    run("refund_bond", f::check_refund_bond);
}

#[test]
fn facility() {
    run("facility", f::check_facility);
}
