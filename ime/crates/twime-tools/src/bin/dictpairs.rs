//! EMOJI (EMOJI-plan 2.2): checks the reading dictionary that `emoji/gen_emoji.py` rebuilt in Python against the
//! engine's own (`Dictionary::parse` + `apply_aliases`). Read only; prints counts.
//!
//!   dictpairs <data.txt> <aliases.txt> <pairs.tsv> [han.tsv]
//! pairs.tsv (`gen_emoji.py --dump-pairs`): `reading<TAB>word` for every entry the script holds. The two sets must be equal:
//! every pair is found in the engine's dictionary and the engine holds no more (entry counts equal). han.tsv
//! (`--dump-han`): `char<TAB>0|1`, the script's Han test, which must agree with the engine's `is_han` on every character.

use std::{fs, process::exit};

use twime_model::lexicon::{Dictionary, is_han};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: dictpairs <data.txt> <aliases.txt> <pairs.tsv> [han.tsv]");
        exit(2);
    }
    let read = |p: &str| fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let (mut dict, stats) = Dictionary::parse(&read(&a[1]));
    let alias = dict.apply_aliases(&read(&a[2])).expect("alias table");
    let engine_total = stats.kept + alias.added;
    let (mut seen, mut missing) = (0usize, 0usize);
    for line in read(&a[3]).lines().filter(|l| !l.is_empty()) {
        let (reading, word) = line.split_once('\t').expect("reading<TAB>word");
        let syl: Vec<String> = reading.split('-').map(String::from).collect();
        seen += 1;
        if !dict.lookup(&syl).iter().any(|e| e.word == word) {
            missing += 1;
        }
    }
    println!("engine entries {engine_total} (parsed {} + aliases {}), script pairs {seen}, not in the engine {missing}", stats.kept, alias.added);
    let mut bad = missing != 0 || seen != engine_total;
    if let Some(han) = a.get(4) {
        let (mut n, mut diff) = (0usize, 0usize);
        for line in read(han).lines().filter(|l| !l.is_empty()) {
            let (c, flag) = line.split_once('\t').expect("char<TAB>flag");
            let c = c.chars().next().expect("char");
            n += 1;
            if is_han(c) != (flag == "1") {
                diff += 1;
                println!("is_han differs: U+{:04X} script={flag}", c as u32);
            }
        }
        println!("han test: {n} characters, {diff} differ");
        bad |= diff != 0;
    }
    println!("{}", if bad { "FAIL" } else { "PASS" });
    if bad {
        exit(1);
    }
}
