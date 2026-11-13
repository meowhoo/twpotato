//! Platform-independent conversion model for the Taiwan bopomofo IME (N3-plan M1).
//!
//! Contains the dictionary / legal-character tables, the single-pass N1/G1 forward (`n1`), the character n-gram
//! prior (`prior`) and `Assets`, the one place that turns `[context][syllables]` into characters.

pub mod cache;
pub mod cand;
pub mod cands;
pub mod convert;
pub mod lexux;
mod kernel;
pub mod qkernel;
pub mod lexicon;
pub mod n1;
pub mod norm;
pub mod pn;
mod repair_api;
pub mod fuzzy_cands;
pub mod autocomp;
pub mod emoji;
pub mod genw;
pub mod prior;
pub mod vocab;

pub use cands::{CandParams, Candidate, WordHit, WordSource};
pub use norm::NormCfg;
pub use convert::{Assets, Conversion, ModelPaths};
#[cfg(feature = "arch-prof")] pub mod prof;

pub mod user;
pub mod wordbonus;

pub mod lm;

pub mod pop;
