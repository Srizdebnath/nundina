//! Builds the program under test before the suite compiles (#169).
//!
//! The tests used to load the repo root's `target/deploy/credit_gate.so`.
//! That file is whatever the last `anchor build` left, so a checkout that
//! never ran it, or ran it before the last `src/` change, loaded a stale
//! binary and failed with `InstructionFallbackNotFound`. Now this script
//! runs `cargo build-sbf` into its own `OUT_DIR` and the harness loads
//! exactly that file: cargo reruns the script whenever the program's
//! source, manifest or the root lockfile changes, and nothing else writes
//! there. `cargo build-sbf` is incremental, so a rerun is cheap.
//!
//! `CREDIT_GATE_SO=<path>` skips the build and loads that .so instead
//! (e.g. a `solana-verify` build being checked before a deploy).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let program = here.parent().unwrap().to_path_buf();
    let root = program.parent().unwrap().parent().unwrap().to_path_buf();
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    for p in [
        program.join("src"),
        program.join("Cargo.toml"),
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        root.join("programs/credit-math"),
    ] {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    println!("cargo:rerun-if-env-changed=CREDIT_GATE_SO");

    // The harness loads `CREDIT_GATE_SO` (tests/common/mod.rs).
    let so = match std::env::var("CREDIT_GATE_SO") {
        Ok(prebuilt) => PathBuf::from(prebuilt),
        Err(_) => {
            build_sbf(&program.join("Cargo.toml"), &out);
            out.join("credit_gate.so")
        }
    };
    println!("cargo:rustc-env=CREDIT_GATE_SO={}", so.display());
}

fn build_sbf(manifest: &Path, out: &Path) {
    // The outer cargo exports variables pointing at the host toolchain
    // (RUSTC, rustup's pinned 1.98.1, rustflags). cargo-build-sbf must
    // use platform-tools instead, so none of them may leak in.
    let mut cmd = Command::new("cargo");
    // v1.52 = the platform-tools `anchor build` 1.1.2 uses; the
    // cargo-build-sbf 2.3 default (v1.48, cargo 1.84) cannot parse
    // edition-2024 dependencies. Same tools → byte-identical .so.
    cmd.arg("build-sbf")
        .arg("--tools-version")
        .arg("v1.52")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--sbf-out-dir")
        .arg(out);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CARGO_") && k != "CARGO_HOME" || k.starts_with("RUSTC") {
            cmd.env_remove(&k);
        }
    }
    for k in [
        "RUSTUP_TOOLCHAIN",
        "RUSTDOC",
        "RUSTFLAGS",
        "TARGET",
        "HOST",
        "OUT_DIR",
    ] {
        cmd.env_remove(k);
    }
    let status = cmd.status().unwrap_or_else(|e| {
        panic!("cannot run `cargo build-sbf` ({e}): install solana-cli, or point CREDIT_GATE_SO at a built .so")
    });
    assert!(
        status.success(),
        "cargo build-sbf failed for {}",
        manifest.display()
    );
}
