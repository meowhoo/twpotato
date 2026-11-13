//! Token ids: UNK, SEP, Han characters, then syllables.
//!
//! Copied from bench/s3_latency/src/model.rs `Vocab` (N3 M1, MIT, same project); `encode` for the old
//! candidate model is not carried over.

use std::collections::HashMap;

use crate::lexicon::Dictionary;

pub const UNK: u32 = 0;
pub const SEP: u32 = 1;

pub struct Vocab {
    chars: HashMap<char, u32>,
    char_list: Vec<char>, // id - 2 -> char
    syllables: HashMap<String, u32>,
    syl_list: Vec<String>, // id - 2 - chars -> syllable
    size: usize,
}

impl Vocab {
    pub fn new(dict: &Dictionary) -> Vocab {
        let (chars, syllables) = dict.vocabulary();
        let mut next = 2u32;
        let mut id = || {
            let v = next;
            next += 1;
            v
        };
        let char_list = chars.clone();
        let syl_list = syllables.clone();
        let chars: HashMap<char, u32> = chars.into_iter().map(|c| (c, id())).collect();
        let syllables: HashMap<String, u32> = syllables.into_iter().map(|s| (s, id())).collect();
        let size = 2 + chars.len() + syllables.len();
        Vocab { chars, char_list, syllables, syl_list, size }
    }
    /// The character of a character token id.
    pub fn char_of(&self, id: u32) -> Option<char> {
        self.char_list.get((id as usize).checked_sub(2)?).copied()
    }
    pub fn len(&self) -> usize {
        self.size
    }
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
    pub fn syllable_id(&self, s: &str) -> u32 {
        *self.syllables.get(s).unwrap_or(&UNK)
    }
    /// The characters in id order (character id = index + 2). PRED: the LM vocabulary is exported from here.
    pub fn chars(&self) -> &[char] {
        &self.char_list
    }
    /// The syllable of a syllable token id (AUTOCOMP prefix lookups).
    pub fn syllable_of(&self, id: u32) -> Option<&str> {
        self.syl_list.get((id as usize).checked_sub(2 + self.char_list.len())?).map(String::as_str)
    }
    pub fn char_id(&self, c: char) -> u32 {
        *self.chars.get(&c).unwrap_or(&UNK)
    }
}
