//! Sets `HH_BUILD_ID`: a hash of the sources that decide what a normalized file and its catalog
//! look like (core sources and model, this crate's sources, the lock file). The disk cache keys
//! its entries by it, so a changed build never reads an older build's results.

use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let base = here.join("..").canonicalize().unwrap_or_else(|_| here.join(".."));
    let inputs = [base.join("agent/src"), base.join("core/src"), base.join("core/models"), base.join("Cargo.lock")];
    let mut all = Vec::new();
    for i in &inputs {
        println!("cargo:rerun-if-changed={}", i.display());
        if i.is_dir() {
            files(i, &mut all);
        } else if i.is_file() {
            all.push(i.clone());
        }
    }
    all.sort();
    // FNV-1a, 64 bit, twice with different seeds (no dependencies in a build script).
    let mut h = [0xcbf2_9ce4_8422_2325u64, 0x6c62_272e_07bb_0142u64];
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            for x in h.iter_mut() {
                *x ^= u64::from(b);
                *x = x.wrapping_mul(0x0100_0000_01b3);
            }
        }
    };
    for p in &all {
        let rel = p.strip_prefix(&base).or_else(|_| p.strip_prefix(&here)).unwrap_or(p).to_string_lossy().replace('\\', "/");
        feed(rel.as_bytes());
        feed(&[0]);
        if let Ok(b) = std::fs::read(p) {
            feed(&(b.len() as u64).to_le_bytes());
            feed(&b);
        }
    }
    println!("cargo:rustc-env=HH_BUILD_ID={:016x}{:016x}", h[0], h[1]);
}
