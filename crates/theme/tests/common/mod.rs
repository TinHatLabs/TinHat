//! Shared test support: SHA-256 (sha256.rs, also compiled into the crate's own unit tests), the mulberry32 PRNG of the
//! reference's unit tests, and file helpers.
#![allow(dead_code)]

mod sha256;
pub use sha256::sha256_hex;

use std::fs;
use std::path::{Path, PathBuf};
use tinhat_theme::{parse_json, TokensSpec};

/// The reference tests' deterministic PRNG (mulberry32), bit for bit: returns doubles in [0, 1).
pub struct Mulberry32(u32);

impl Mulberry32 {
    pub fn new(seed: u32) -> Mulberry32 {
        Mulberry32(seed)
    }
    pub fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        ((t ^ (t >> 14)) as f64) / 4294967296.0
    }
    /// `Math.floor(r() * n)`
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() * n as f64).floor() as usize
    }
}

pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn vec_dir() -> PathBuf {
    crate_dir().join("testvectors")
}

/// The real token list the vectors use (`engine-tokens.json` = contract tokens.json).
pub fn vector_spec() -> TokensSpec {
    let text = fs::read_to_string(vec_dir().join("engine-tokens.json")).expect("engine-tokens.json");
    TokensSpec::from_json(&parse_json(&text).expect("engine-tokens.json is JSON")).expect("the vector token spec")
}

pub fn sorted_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir).expect("dir").map(|e| e.expect("entry").path()).collect();
    v.sort();
    v
}

pub fn read_json(p: &Path) -> serde_json::Value {
    parse_json(&fs::read_to_string(p).expect("read")).expect("JSON")
}
