//! PN latency scenarios over the real engine pipe. Counts and times only, never text.
//!
//!   pnlat run <pipe> <units> <docs> <dense.txt> [--cpu-mask HEX] [--engine-pid P] [--n 600] [--conf]
//!
//! Scenarios (each key's full round trip through the pipe, ms):
//!   cold      the first 5 dev units right after the engine started (first touches of model, prior and lexicon pages)
//!   cand      after typing a dev unit: Down (open the list at the end), digit 2 (select item 2: lock + recompute),
//!             Left x3 + Down (list in the middle), Esc
//!   edit      longest dev unit: Home, then a new syllable typed at the front (whole composition recomputed), then
//!             Backspace at the front; repeated
//!   dense     syllable lines of dense.txt (readings with many lexicon words, chained to 39 syllables), typed in full
//! dense.txt: one composition per line, syllables separated by spaces (made by ent_pn_eval.py dense).
//!
//! FUZZY F1 (FUZZY-plan 3.7 gate 5), an engine started with `--repair auto|suggest`:
//!   pnlat repair <pipe> <lat-manifest.json> --mode auto|suggest [--lexicon yes|no] [--cpu-mask HEX] [--engine-pid P]
//! Per manifest unit one state chain on one session: `SetContext`, every syllable typed with the repaired position typed
//! as `typed` (A: the key completing it), Left to just after it, Down (B), PageDown to the page of the first item of
//! another repair reading (`suggest`: the first item of the repair list) and its number key (C: choice + new inference),
//! Home, the front syllable (D1: its completing key), Backspace (D2), Esc. 100 warm-up keys first (not recorded). Each
//! observation is kept only when the composition / list shows the repair at work (plan 3.7 "valid observations"); the
//! counts of valid observations are printed; fewer than 500 per operation = invalid round (exit 3).
//!
//! CACHE: the connection first negotiates with `Hello`; on version 4 (an engine with the document
//! cache) every composition of cand / edit / dense starts with a non-empty `DocText`, and the scenarios doc (the unit's own
//! document before it, at most 8192 UTF-16 units), doc-full (always 8192 units), doc-hit (dense lines, snapshot = the line's
//! output without cache) and doc-fail (empty snapshot) are added. `*_first` = `DocText` round trip + the first key of the
//! composition as one observation (on version 3 the first key alone). Memory lines after every scenario.
//!
//! CACHE-c, `--conf` (the engine runs `cache_source=confirmed`): one session, never
//! `SetContext` (it would forget the confirmations). "Prime" a unit = `ExternalEdit` (empty model context, confirmations
//! kept), snapshot = the committed text so far, type it, note the composition, choose item 2 (else 1) with a number key at
//! its last two positions, Enter (timed: conf_commit). Before that, a baseline typing after `ExternalEdit` without any
//! snapshot (no cache) is cancelled. Then the timed typing of the same unit after `ExternalEdit` with the new snapshot: the
//! model context is empty in both, so a composition whose last two characters are the chosen ones and differ from the
//! baseline is a measured cache hit (`*_hits`, counted per scenario; at least 1 each, else the run is invalid).
//! conf-cold also reports the engine's first composition after the start (`confcold_start_*`, no cache) and the typing of
//! the first five primes (`confcold_prime_*`), judged like r6 cold.
//!   conf-fill    commits without snapshots until more distinct runs than MAX_RUNS were chosen (capacity and eviction)
//!   conf-cold    the first 5 units: prime + timed typing
//!   conf-cand    n units: prime + timed typing + open at end, select item 2, Left x3, open in the middle
//!   conf-edit    longest unit (38 syllables): prime + timed typing + Home, front insert, Backspace
//!   conf-dense   dense lines: prime + timed typing + open at end
//!   conf-full    the conf-cand units again with the last 8192 units of the committed text as snapshot (timed)

//! EMOJI (EMOJI-plan 5 V3): `pnlat emoji <pipe> <manifest.json> --tag on|off`, see `emoji_lat`.
//! ZHUYIN-OUT (ZHUYIN-OUT-plan 7): `pnlat zyout <pipe> <manifest.json> --tag base|new`, see `zyout_lat`.

#[path = "pnlat/pop.rs"]
mod pop;

use std::time::Instant;

use twime_core::keymap::key_of;
use twime_proto::{Event, Key, KeyEvent, PROTOCOL_VERSION, Reply, Request, Update};
use twime_tools::cases::{Case, load_cases};
use twime_winpipe::{CallError, Client};

const LONGEST_ID: &str = "ey-be4cabb8e5df-038";

struct Conn {
    c: Client,
    seq: u32,
}

impl Conn {
    fn call(&mut self, session: u32, event: Event) -> Result<Update, CallError> {
        self.seq += 1;
        let r = Request { session, seq: self.seq, ack: None, event };
        let bytes = self.c.call(&r.encode(), 5000)?;
        match Reply::decode(&bytes) {
            Some(Reply::Update { update, .. }) => Ok(update),
            _ => Err(CallError::Broken(std::io::Error::other("unexpected reply"))),
        }
    }
    fn hello(&mut self) -> (u16, bool) {
        let r = Request { session: 0, seq: 0, ack: None, event: Event::Hello(PROTOCOL_VERSION) };
        let bytes = self.c.call(&r.encode(), 5000).expect("hello");
        hello_info(Reply::decode(&bytes).expect("Hello reply")).expect("no Hello reply")
    }
    fn timed(&mut self, session: u32, k: Key, v: &mut Vec<f64>) {
        let t = Instant::now();
        self.call(session, Event::Key(KeyEvent::press(k))).expect("key");
        v.push(t.elapsed().as_secs_f64() * 1000.0);
    }
}

// USERLEX version 5 can advertise no document-text capability (e.g. cache off).
// Preserve the negotiated version while deciding the scenarios from the actual capability.
fn hello_info(reply: Reply) -> Option<(u16, bool)> {
    match reply {
        Reply::Hello { version } => Some((version, version >= 4)),
        Reply::HelloCaps { version, caps } => Some((version, caps & twime_proto::CAP_DOC_TEXT != 0)),
        _ => None,
    }
}

fn line(name: &str, mut ms: Vec<f64>) {
    // CACHE plan G10: with PNLAT_RAW=<file> every observation is appended there (name, ms) so rounds can be pooled.
    if let Ok(p) = std::env::var("PNLAT_RAW")
        && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p)
    {
        use std::io::Write;
        for v in &ms {
            let _ = writeln!(f, "{name}	{v:.4}");
        }
    }
    if ms.is_empty() {
        println!("{name}\tn=0");
        return;
    }
    ms.sort_by(f64::total_cmp);
    let p = |q: f64| ms[((ms.len() as f64 * q) as usize).min(ms.len() - 1)];
    println!("{name}\tn={}\tp50={:.3}\tp95={:.3}\tp99={:.3}\tmax={:.3}\t(ms)", ms.len(), p(0.5), p(0.95), p(0.99), ms[ms.len() - 1]);
}

fn syllable_keys(s: &str) -> Option<Vec<Key>> {
    let mut keys = vec![];
    let mut tone = false;
    for c in s.chars() {
        keys.push(key_of(c)?);
        tone = matches!(c, 'ˊ' | 'ˇ' | 'ˋ' | '˙');
    }
    if !tone {
        keys.push(Key::Space);
    }
    Some(keys)
}

/// Types the syllables; trigger keys (the last key of a syllable) go to `trig`, the others to `other`.
fn type_syl(conn: &mut Conn, session: u32, syl: &[String], trig: &mut Vec<f64>, other: &mut Vec<f64>) -> bool {
    let Some(seqs): Option<Vec<Vec<Key>>> = syl.iter().map(|s| syllable_keys(s)).collect() else { return false };
    for keys in seqs {
        for (j, k) in keys.iter().enumerate() {
            if j + 1 == keys.len() {
                conn.timed(session, *k, trig);
            } else {
                conn.timed(session, *k, other);
            }
        }
    }
    true
}

/// Types the syllables of one composition; with `doc` (version 4) the `DocText` round trip and the first key are one
/// observation in `first`, as the shell does it (CACHE plan 3.2); without, the first key alone goes to `first`.
fn type_doc(conn: &mut Conn, session: u32, syl: &[String], doc: Option<&str>, first: &mut Vec<f64>, trig: &mut Vec<f64>, other: &mut Vec<f64>) -> bool {
    let Some(seqs): Option<Vec<Vec<Key>>> = syl.iter().map(|s| syllable_keys(s)).collect() else { return false };
    let mut is_first = true;
    for keys in seqs {
        for (j, k) in keys.iter().enumerate() {
            if is_first {
                let t = Instant::now();
                if let Some(d) = doc {
                    conn.call(session, Event::DocText(d.to_string())).expect("doctext");
                }
                conn.call(session, Event::Key(KeyEvent::press(*k))).expect("key");
                first.push(t.elapsed().as_secs_f64() * 1000.0);
                is_first = false;
            } else if j + 1 == keys.len() {
                conn.timed(session, *k, trig);
            } else {
                conn.timed(session, *k, other);
            }
        }
    }
    true
}

/// Last `units` UTF-16 units of a text (what the shell sends).
fn tail16(t: &str, units: usize) -> String {
    let v: Vec<u16> = t.encode_utf16().collect();
    String::from_utf16_lossy(&v[v.len().saturating_sub(units)..])
}

/// The document text before each dev unit: the alignment of `load_cases` (every dev unit advances the cursor, scored or
/// not); a unit not found or a snapshot whose end differs from the case context is an error.
fn prefixes(cases: &[Case], units: &str, docs: &str) -> Vec<String> {
    let mut text = std::collections::HashMap::new();
    for l in std::fs::read_to_string(docs).expect("docs").lines() {
        let v: serde_json::Value = serde_json::from_str(l).expect("doc");
        text.insert(v["doc_id"].as_str().unwrap().to_string(), v["text"].as_str().unwrap().to_string());
    }
    let mut cur: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut at: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for l in std::fs::read_to_string(units).expect("units").lines() {
        let u: serde_json::Value = serde_json::from_str(l).expect("unit");
        if u["split"] != "dev" {
            continue;
        }
        let (doc, ut) = (u["doc_id"].as_str().unwrap().to_string(), u["text"].as_str().unwrap());
        let t: &String = &text[&doc];
        let from = *cur.get(&doc).unwrap_or(&0);
        let pos = from + t[from..].find(ut).expect("dev unit not found in its document");
        cur.insert(doc, pos + ut.len());
        if u["status"] == "ok" {
            at.insert(u["id"].as_str().unwrap().to_string(), tail16(&t[..pos], twime_proto::DOC_TEXT_UNITS));
        }
    }
    cases
        .iter()
        .map(|c| {
            let p = at.remove(&c.id).expect("case without a position");
            assert!(p.ends_with(&c.context), "snapshot end differs from the case context");
            p
        })
        .collect()
}

fn arg(a: &[String], f: &str) -> Option<String> {
    a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned()
}

fn mem(pid: Option<u32>, tag: &str) {
    if let Some(p) = pid
        && let Ok((ws, private)) = twime_winpipe::memory_of_pid(p)
    {
        println!("{tag}\tpid={p}\tworking_set_mb={:.1}\tprivate_mb={:.1}", ws as f64 / 1048576.0, private as f64 / 1048576.0);
    }
}

/// The composition text after the last key (a `Ping` returns the current update).
fn comp_text(conn: &mut Conn, session: u32) -> String {
    conn.call(session, Event::Ping).ok().and_then(|u| u.composition).map(|c| c.text()).unwrap_or_default()
}

/// Opens the list after position `j` and chooses item 2 (item 1 when there is one item) with a number key.
/// Opens the list after position `j` and chooses with a number key, on the first page, the single character equal to the
/// current one (`alt` None: confirms it) or the k-th single character that differs (`alt` Some(k), k taken modulo their
/// number); none: Esc.
fn choose_at(conn: &mut Conn, session: u32, j: usize, alt: Option<usize>) {
    let cur = comp_text(conn, session).chars().nth(j);
    let _ = conn.call(session, Event::Key(KeyEvent::press(Key::Home)));
    for _ in 0..=j {
        let _ = conn.call(session, Event::Key(KeyEvent::press(Key::Right)));
    }
    let items = conn.call(session, Event::Key(KeyEvent::press(Key::Down))).ok().and_then(|u| u.candidates).map(|c| c.items).unwrap_or_default();
    let single = |it: &String| {
        let mut cs = it.chars();
        let c = cs.next();
        if cs.next().is_some() { None } else { c }
    };
    let pick = match alt {
        None => items.iter().position(|it| single(it).is_some() && single(it) == cur),
        Some(k) => {
            let diff: Vec<usize> = (0..items.len()).filter(|&i| single(&items[i]).is_some() && single(&items[i]) != cur).collect();
            (!diff.is_empty()).then(|| diff[k % diff.len()])
        }
    };
    match pick {
        Some(i) if i < 9 => {
            let _ = conn.call(session, Event::Key(KeyEvent::press(Key::Digit(i as u8 + 1))));
        }
        _ => {
            let _ = conn.call(session, Event::Key(KeyEvent::press(Key::Esc)));
        }
    }
    let _ = conn.call(session, Event::Key(KeyEvent::press(Key::End)));
}

/// Maximum confirmed runs of the engine (twime_model::cache::MAX_RUNS).
const MAX_RUNS: usize = twime_model::cache::MAX_RUNS;

struct ConfRun {
    committed: String,
    commit_ms: Vec<f64>,
    /// Commit keys pressed, and how many did not commit.
    primes: usize,
    failed: usize,
    /// Every committed prime in order (syllables, (baseline, chosen)), for conf-full.
    history: Vec<(Vec<String>, (String, String))>,
    /// Which differing character the next prime chooses at the last position (always the first one).
    alt: usize,
}

impl ConfRun {
    /// Prime one unit (see the header): returns the last two characters without any cache (the baseline: typed after
    /// `ExternalEdit` with no snapshot, then cancelled) and the committed ones after the choices (None: not committed). The
    /// typing of the prime goes to `pt` (first, trigger, other).
    fn prime(&mut self, conn: &mut Conn, s: u32, syl: &[String], snapshot: bool, pt: &mut [Vec<f64>; 3]) -> Option<(String, String)> {
        if syl.len() < 2 {
            return None;
        }
        // baseline without cache (no DocText: no snapshot set)
        conn.call(s, Event::ExternalEdit).expect("external edit");
        let (mut a, mut b, mut c) = (vec![], vec![], vec![]);
        if !type_doc(conn, s, syl, None, &mut a, &mut b, &mut c) {
            let _ = conn.call(s, Event::Key(KeyEvent::press(Key::Esc)));
            return None;
        }
        let base: String = comp_text(conn, s).chars().rev().take(2).collect();
        let _ = conn.call(s, Event::Key(KeyEvent::press(Key::Esc)));
        conn.call(s, Event::ExternalEdit).expect("external edit");
        let d = snapshot.then(|| tail16(&self.committed, twime_proto::DOC_TEXT_UNITS));
        let [pf, ptg, po] = pt;
        if !type_doc(conn, s, syl, d.as_deref(), pf, ptg, po) {
            let _ = conn.call(s, Event::Key(KeyEvent::press(Key::Esc)));
            return None;
        }
        // confirm the character before the last, choose another character for the last: a run of two whose pair the
        // cache can raise against the model's first choice
        choose_at(conn, s, syl.len() - 2, None);
        choose_at(conn, s, syl.len() - 1, Some(self.alt));
        let t = Instant::now();
        let u = conn.call(s, Event::Key(KeyEvent::press(Key::Enter))).expect("key");
        self.commit_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        self.primes += 1;
        self.failed += usize::from(u.commit.is_none());
        match u.commit {
            Some(text) => {
                self.committed.push_str(&text);
                let p = (base, text.chars().rev().take(2).collect::<String>());
                self.history.push((syl.to_vec(), p.clone()));
                Some(p)
            }
            None => {
                let _ = conn.call(s, Event::Key(KeyEvent::press(Key::Esc)));
                None
            }
        }
    }

    /// The timed typing after a prime; true when the end of the composition is the chosen characters and differs from the
    /// baseline without cache (only the cache can have changed it).
    fn timed(&mut self, conn: &mut Conn, s: u32, syl: &[String], primed: &(String, String), f: &mut Vec<f64>, t: &mut Vec<f64>, o: &mut Vec<f64>) -> bool {
        conn.call(s, Event::ExternalEdit).expect("external edit");
        let d = tail16(&self.committed, twime_proto::DOC_TEXT_UNITS);
        type_doc(conn, s, syl, Some(&d), f, t, o);
        let now: String = comp_text(conn, s).chars().rev().take(2).collect();
        primed.0 != primed.1 && now == primed.1
    }
}

/// Observations one composition gives in `type_doc`: (first, trigger, other).
fn obs(syl: &[String]) -> (usize, usize, usize) {
    let seqs: Vec<Vec<Key>> = syl.iter().map(|s| syllable_keys(s).expect("typable")).collect();
    let (mut f, mut t, mut o, mut first) = (0, 0, 0, true);
    for keys in &seqs {
        for j in 0..keys.len() {
            if first {
                f += 1;
                first = false;
            } else if j + 1 == keys.len() {
                t += 1;
            } else {
                o += 1;
            }
        }
    }
    (f, t, o)
}

/// Plan G10 validity (CACHE-c): observation counts derived from the input, hits, eviction; collected reasons.
#[derive(Default)]
struct Validity(Vec<String>);

impl Validity {
    fn count(&mut self, name: &str, got: usize, want: usize) {
        if got != want {
            self.0.push(format!("{name} n={got} expected {want}"));
        }
    }
    fn group(&mut self, name: &str, v: &[Vec<f64>; 3], units: &[&[String]]) {
        let (mut f, mut t, mut o) = (0, 0, 0);
        for u in units {
            let x = obs(u);
            f += x.0;
            t += x.1;
            o += x.2;
        }
        self.count(&format!("{name}_first"), v[0].len(), f);
        self.count(&format!("{name}_trigger"), v[1].len(), t);
        self.count(&format!("{name}_other"), v[2].len(), o);
    }
    fn hits(&mut self, name: &str, hits: usize) {
        println!("{name}_hits\t{hits}");
        if hits == 0 {
            self.0.push(format!("{name}: no measured cache hit"));
        }
    }
}

fn lines3(name: &str, v: [Vec<f64>; 3]) {
    let [f, t, o] = v;
    line(&format!("{name}_first"), f);
    line(&format!("{name}_trigger"), t);
    line(&format!("{name}_other"), o);
}

fn conf(conn: &mut Conn, cases: &[Case], dense: &[Vec<String>], n: usize, step: usize, pid: Option<u32>) -> bool {
    const S: u32 = 6;
    let esc = |conn: &mut Conn| {
        let _ = conn.call(S, Event::Key(KeyEvent::press(Key::Esc)));
        let _ = conn.call(S, Event::Key(KeyEvent::press(Key::Esc)));
    };
    // units: at least two syllables (two positions to choose) and typable; fixed by the input, as the counts derived from it
    let usable = |s: &[String]| s.len() >= 2 && s.iter().all(|x| syllable_keys(x).is_some());
    let pool: Vec<usize> = (0..cases.len()).filter(|&i| usable(&cases[i].symbols)).collect();
    let mut v = Validity::default();
    let mut r = ConfRun { committed: String::new(), commit_ms: vec![], primes: 0, failed: 0, history: vec![], alt: 0 };
    // conf-cold: the engine's first composition (no cache), the typing of the first five primes, their timed retyping
    let cold_units: Vec<usize> = pool.iter().copied().take(5).collect();
    let mut start: [Vec<f64>; 3] = Default::default();
    {
        conn.call(S, Event::ExternalEdit).expect("external edit");
        let [a, b, c] = &mut start;
        type_doc(conn, S, &cases[cold_units[0]].symbols, None, a, b, c);
        esc(conn);
    }
    let (mut prime_t, mut timed_t): ([Vec<f64>; 3], [Vec<f64>; 3]) = Default::default();
    let mut hits = 0;
    for &i in &cold_units {
        if let Some(p) = r.prime(conn, S, &cases[i].symbols, true, &mut prime_t) {
            let [a, b, c] = &mut timed_t;
            hits += usize::from(r.timed(conn, S, &cases[i].symbols, &p, a, b, c));
        }
        esc(conn);
    }
    let cu: Vec<&[String]> = cold_units.iter().map(|&i| cases[i].symbols.as_slice()).collect();
    v.group("confcold_start", &start, &cu[..1]);
    v.group("confcold_prime", &prime_t, &cu);
    v.group("confcold", &timed_t, &cu);
    v.hits("confcold", hits);
    lines3("confcold_start", start);
    lines3("confcold_prime", prime_t);
    lines3("confcold", timed_t);
    // conf-fill: no snapshots (nothing is dropped), until more distinct runs than the bound were chosen
    let mut distinct = std::collections::HashSet::new();
    for &i in &pool {
        if distinct.len() > MAX_RUNS + 50 {
            break;
        }
        if let Some((_, chosen)) = r.prime(conn, S, &cases[i].symbols, false, &mut Default::default()) {
            distinct.insert(chosen);
        }
    }
    println!("conffill_distinct_runs\t{}\tbound\t{MAX_RUNS}", distinct.len());
    if distinct.len() <= MAX_RUNS {
        v.0.push(format!("conf-fill: {} distinct runs, not above the bound {MAX_RUNS}", distinct.len()));
    }
    mem(pid, "memory_after_conf_fill");
    // conf-cand
    let cand: Vec<usize> = pool.iter().copied().step_by(step).take(n).collect();
    let mut tt: [Vec<f64>; 3] = Default::default();
    let (mut down_end, mut select, mut left, mut down_mid, mut hits) = (vec![], vec![], vec![], vec![], 0);
    let mut primed = vec![];
    for &i in &cand {
        let Some(p) = r.prime(conn, S, &cases[i].symbols, true, &mut Default::default()) else {
            esc(conn);
            continue;
        };
        let [a, b, c] = &mut tt;
        hits += usize::from(r.timed(conn, S, &cases[i].symbols, &p, a, b, c));
        primed.push((i, p));
        conn.timed(S, Key::Down, &mut down_end);
        conn.timed(S, Key::Digit(2), &mut select);
        for _ in 0..3 {
            conn.timed(S, Key::Left, &mut left);
        }
        conn.timed(S, Key::Down, &mut down_mid);
        esc(conn);
    }
    let cu: Vec<&[String]> = cand.iter().map(|&i| cases[i].symbols.as_slice()).collect();
    v.group("confcand", &tt, &cu);
    for (name, got, want) in [("confcand_open_at_end", down_end.len(), cand.len()), ("confcand_select", select.len(), cand.len()), ("confcand_left", left.len(), 3 * cand.len()), ("confcand_open_mid", down_mid.len(), cand.len())] {
        v.count(name, got, want);
    }
    v.hits("confcand", hits);
    lines3("confcand", tt);
    line("confcand_open_at_end", down_end);
    line("confcand_select_item2_lock_recompute", select);
    line("confcand_cursor_left", left);
    line("confcand_open_in_middle", down_mid);
    mem(pid, "memory_after_conf_cand");
    // conf-edit: 38-syllable compositions (the first 38 syllables of the dense lines, one per repetition: the single longest
    // dev unit never showed a measurable hit), Home, a syllable typed at the front (whole composition recomputed), Backspace
    let reps = (n / 4).max(20);
    let edits: Vec<Vec<String>> = (0..reps).map(|k| dense[k % dense.len()].iter().take(38).cloned().collect()).collect();
    let mut tt: [Vec<f64>; 3] = Default::default();
    let (mut home, mut ins_t, mut ins_o, mut bs, mut hits) = (vec![], vec![], vec![], vec![], 0);
    let mut fronts = vec![];
    for (k, syl) in edits.iter().enumerate() {
        let Some(p) = r.prime(conn, S, syl, true, &mut Default::default()) else {
            esc(conn);
            continue;
        };
        let [a, b, c] = &mut tt;
        hits += usize::from(r.timed(conn, S, syl, &p, a, b, c));
        conn.timed(S, Key::Home, &mut home);
        let front = vec![syl[k % syl.len()].clone()];
        type_syl(conn, S, &front, &mut ins_t, &mut ins_o);
        fronts.push(front);
        conn.timed(S, Key::Backspace, &mut bs);
        esc(conn);
    }
    v.group("confedit", &tt, &edits.iter().map(|x| x.as_slice()).collect::<Vec<_>>());
    v.count("confedit_home", home.len(), reps);
    v.count("confedit_backspace", bs.len(), reps);
    v.count("confedit_front_insert_trigger", ins_t.len(), fronts.len());
    v.hits("confedit", hits);
    lines3("confedit", tt);
    line("confedit_home", home);
    line("confedit_front_insert_trigger", ins_t);
    line("confedit_front_backspace", bs);
    mem(pid, "memory_after_conf_edit");
    // conf-dense
    let dl: Vec<&Vec<String>> = (0..reps).map(|k| &dense[k % dense.len()]).collect();
    let mut tt: [Vec<f64>; 3] = Default::default();
    let (mut dd, mut hits) = (vec![], 0);
    for d in &dl {
        let Some(p) = r.prime(conn, S, d, true, &mut Default::default()) else {
            esc(conn);
            continue;
        };
        let [a, b, c] = &mut tt;
        hits += usize::from(r.timed(conn, S, d, &p, a, b, c));
        conn.timed(S, Key::Down, &mut dd);
        esc(conn);
    }
    v.group("confdense", &tt, &dl.iter().map(|d| d.as_slice()).collect::<Vec<_>>());
    v.count("confdense_open_at_end", dd.len(), reps);
    v.hits("confdense", hits);
    lines3("confdense", tt);
    line("confdense_cand_open_at_end", dd);
    mem(pid, "memory_after_conf_dense");
    // conf-full: the last 150 primed units again, most recent first (their runs are still in the window), snapshot = the last
    // 8192 units of everything committed
    let mut tt: [Vec<f64>; 3] = Default::default();
    let mut hits = 0;
    let recent: Vec<(Vec<String>, (String, String))> = r.history.iter().rev().take(reps).cloned().collect();
    for (syl, p) in &recent {
        let [a, b, c] = &mut tt;
        hits += usize::from(r.timed(conn, S, syl, p, a, b, c));
        esc(conn);
    }
    v.group("conffull", &tt, &recent.iter().map(|(x, _)| x.as_slice()).collect::<Vec<_>>());
    v.hits("conffull", hits);
    lines3("conffull", tt);
    mem(pid, "memory_after_conf_full");
    // every prime of the timed scenarios must have committed (a failure changes the derived counts)
    v.count("conf_commit", r.commit_ms.len(), r.primes);
    if r.failed > 0 {
        v.0.push(format!("{} primes not committed", r.failed));
    }
    line("conf_commit", std::mem::take(&mut r.commit_ms));
    for x in &v.0 {
        println!("conf_invalid\t{x}");
    }
    println!("conf_valid\t{}", u8::from(v.0.is_empty()));
    v.0.is_empty()
}

fn run(a: &[String]) {
    let pipe = &a[2];
    let cases: Vec<Case> = load_cases(&a[3], &a[4]).expect("cases");
    let dense: Vec<Vec<String>> = std::fs::read_to_string(&a[5]).expect("dense").lines().filter(|l| !l.is_empty()).map(|l| l.split(' ').map(String::from).collect()).collect();
    let n: usize = arg(a, "--n").map_or(600, |v| v.parse().unwrap());
    let pid: Option<u32> = arg(a, "--engine-pid").and_then(|v| v.parse().ok());
    if let Some(m) = arg(a, "--cpu-mask") {
        unsafe {
            let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), usize::from_str_radix(&m, 16).unwrap());
        }
    }
    let mut conn = Conn { c: Client::connect(pipe, None, 200).expect("connect"), seq: 0 };
    let (version, doc) = conn.hello();
    println!("version\t{version}");
    let pre = prefixes(&cases, &a[3], &a[4]);
    if a.iter().any(|x| x == "--conf") {
        assert!(doc, "--conf needs an engine with the cache (version 4)");
        mem(pid, "memory_start");
        let ok = conf(&mut conn, &cases, &dense, n, (cases.len() / n).max(1), pid);
        mem(pid, "memory_end");
        if !ok {
            // plan G10: an invalid round (counts, hits, eviction) is rerun, never judged
            std::process::exit(3);
        }
        return;
    }
    let snap = |i: usize| -> Option<String> { doc.then(|| pre[i].clone()) };
    mem(pid, "memory_start");
    // cold: the first units after start (with the cache: the first non-empty snapshot, first set build, first conversion)
    let (mut cf, mut ct, mut co) = (vec![], vec![], vec![]);
    for (i, c) in cases.iter().enumerate().take(5) {
        conn.call(1, Event::SetContext(c.context.clone())).unwrap();
        type_doc(&mut conn, 1, &c.symbols, snap(i).as_deref(), &mut cf, &mut ct, &mut co);
        let _ = conn.call(1, Event::Key(KeyEvent::press(Key::Esc)));
    }
    line("cold_first5units_first", cf);
    line("cold_first5units_trigger", ct);
    line("cold_first5units_other", co);
    mem(pid, "memory_after_cold");
    if arg(a,"--lexux-manifest").is_some() { lexux_warmup(&mut conn,&dense[0]); }
    // cand: open / select / move / open
    let (mut f, mut t, mut o, mut down_end, mut select, mut left, mut down_mid) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    let step = (cases.len() / n).max(1);
    for (i, c) in cases.iter().enumerate().step_by(step).take(n) {
        conn.call(1, Event::SetContext(c.context.clone())).unwrap();
        if !type_doc(&mut conn, 1, &c.symbols, snap(i).as_deref(), &mut f, &mut t, &mut o) {
            continue;
        }
        conn.timed(1, Key::Down, &mut down_end);
        conn.timed(1, Key::Digit(2), &mut select);
        for _ in 0..3 {
            conn.timed(1, Key::Left, &mut left);
        }
        conn.timed(1, Key::Down, &mut down_mid);
        let _ = conn.call(1, Event::Key(KeyEvent::press(Key::Esc)));
        let _ = conn.call(1, Event::Key(KeyEvent::press(Key::Esc)));
    }
    line("dev_first", f);
    line("dev_trigger", t);
    line("dev_other_keys", o);
    line("cand_open_at_end", down_end);
    line("cand_select_item2_lock_recompute", select);
    line("cursor_left", left);
    line("cand_open_in_middle", down_mid);
    mem(pid, "memory_after_cand");
    // edit: longest unit, front insert / delete
    let li = cases.iter().position(|c| c.id == LONGEST_ID).unwrap_or(0);
    let longest = &cases[li];
    let (mut lf, mut lt, mut lo, mut home, mut ins_t, mut ins_o, mut bs) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    for r in 0..(n / 4).max(20) {
        conn.call(2, Event::SetContext(longest.context.clone())).unwrap();
        let syl: Vec<String> = longest.symbols.iter().take(38).cloned().collect();
        type_doc(&mut conn, 2, &syl, snap(li).as_deref(), &mut lf, &mut lt, &mut lo);
        conn.timed(2, Key::Home, &mut home);
        let front = vec![longest.symbols[r % longest.symbols.len()].clone()];
        type_syl(&mut conn, 2, &front, &mut ins_t, &mut ins_o);
        conn.timed(2, Key::Backspace, &mut bs);
        let _ = conn.call(2, Event::Key(KeyEvent::press(Key::Esc)));
    }
    line("longest_first", lf);
    line("longest_trigger", lt);
    line("edit_home", home);
    line("edit_front_insert_trigger", ins_t);
    line("edit_front_backspace", bs);
    mem(pid, "memory_after_edit");
    // a fixed full snapshot (8192 UTF-16 units of dev text)
    let full: String = {
        let mut s = String::new();
        for p in pre.iter().rev() {
            if s.encode_utf16().count() >= twime_proto::DOC_TEXT_UNITS {
                break;
            }
            s = format!("{p}{s}");
        }
        tail16(&s, twime_proto::DOC_TEXT_UNITS)
    };
    // dense
    let (mut df, mut dt, mut dk, mut dd) = (vec![], vec![], vec![], vec![]);
    for r in 0..(n / 4).max(20) {
        let s = &dense[r % dense.len()];
        conn.call(3, Event::SetContext(String::new())).unwrap();
        type_doc(&mut conn, 3, s, doc.then_some(full.as_str()), &mut df, &mut dt, &mut dk);
        conn.timed(3, Key::Down, &mut dd);
        let _ = conn.call(3, Event::Key(KeyEvent::press(Key::Esc)));
        let _ = conn.call(3, Event::Key(KeyEvent::press(Key::Esc)));
    }
    line("dense_first", df);
    line("dense_trigger", dt);
    line("dense_cand_open_at_end", dd);
    mem(pid, "memory_after_dense");
    if doc {
        // doc: each dev unit with its own document before it; doc-full: always 8192 units; doc-fail: empty snapshot
        for (name, kind) in [("doc", 0), ("docfull", 1), ("docfail", 2)] {
            let (mut f, mut t, mut o) = (vec![], vec![], vec![]);
            for (i, c) in cases.iter().enumerate().step_by(step).take(n) {
                conn.call(4, Event::SetContext(c.context.clone())).unwrap();
                let d = match kind {
                    0 => pre[i].clone(),
                    1 => full.clone(),
                    _ => String::new(),
                };
                type_doc(&mut conn, 4, &c.symbols, Some(&d), &mut f, &mut t, &mut o);
                let _ = conn.call(4, Event::Key(KeyEvent::press(Key::Esc)));
            }
            line(&format!("{name}_first"), f);
            line(&format!("{name}_trigger"), t);
            line(&format!("{name}_other"), o);
            mem(pid, &format!("memory_after_{name}"));
        }
        // doc-hit: dense lines, the snapshot is the line's own output without cache (densest hits)
        let (mut f, mut t, mut o, mut dd) = (vec![], vec![], vec![], vec![]);
        for r in 0..(n / 4).max(20) {
            let s = &dense[r % dense.len()];
            conn.call(5, Event::SetContext(String::new())).unwrap();
            let (mut x, mut y, mut z) = (vec![], vec![], vec![]);
            type_doc(&mut conn, 5, s, Some(""), &mut x, &mut y, &mut z);
            let out = match conn.call(5, Event::Key(KeyEvent::press(Key::Down))) {
                Ok(u) => u.composition.map(|c| c.text()).unwrap_or_default(),
                Err(_) => String::new(),
            };
            let _ = conn.call(5, Event::Key(KeyEvent::press(Key::Esc)));
            let _ = conn.call(5, Event::Key(KeyEvent::press(Key::Esc)));
            conn.call(5, Event::SetContext(String::new())).unwrap();
            type_doc(&mut conn, 5, s, Some(&out), &mut f, &mut t, &mut o);
            conn.timed(5, Key::Down, &mut dd);
            let _ = conn.call(5, Event::Key(KeyEvent::press(Key::Esc)));
            let _ = conn.call(5, Event::Key(KeyEvent::press(Key::Esc)));
        }
        line("dochit_first", f);
        line("dochit_trigger", t);
        line("dochit_other", o);
        line("dochit_cand_open_at_end", dd);
        mem(pid, "memory_after_dochit");
    }
    lexux_measure(&mut conn,a,n);
    mem(pid, "memory_end");
}

/// The composition's segments after the last event (a `Ping` returns the current update).
fn segments(conn: &mut Conn, session: u32) -> Vec<twime_proto::Segment> {
    conn.call(session, Event::Ping).ok().and_then(|u| u.composition).map(|c| c.segments).unwrap_or_default()
}

/// The reading shown in a candidate string `text（a、b）` (the last one listed; FUZZY 3.4 display strings).
fn reading_of(item: &str) -> Option<String> {
    let inner = item.strip_suffix('）')?.rsplit_once('（')?.1;
    inner.rsplit('、').next().map(String::from)
}

/// FUZZY F1 gate 5 (see the header).
fn repair(a: &[String]) {
    let pipe = &a[2];
    let man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).expect("manifest")).expect("manifest json");
    let mode = arg(a, "--mode").expect("--mode auto|suggest");
    let auto = match mode.as_str() {
        "auto" => true,
        "suggest" => false,
        _ => panic!("--mode auto|suggest"),
    };
    let tag = if arg(a, "--lexicon").as_deref() == Some("yes") { "lex" } else { "nolex" };
    let pid: Option<u32> = arg(a, "--engine-pid").and_then(|v| v.parse().ok());
    if let Some(m) = arg(a, "--cpu-mask") {
        unsafe {
            let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), usize::from_str_radix(&m, 16).unwrap());
        }
    }
    let mut conn = Conn { c: Client::connect(pipe, None, 200).expect("connect"), seq: 0 };
    println!("version\t{}", conn.hello().0);
    if tag == "lex" {
        // the engine's loaded user lexicon (counts only): the frozen 1,000 words, none invalid, or the round is invalid
        conn.seq += 1;
        let r = Request { session: 0, seq: conn.seq, ack: None, event: Event::User(twime_proto::UserReq::List { query: String::new(), offset: 0, limit: 1 }) };
        let bytes = conn.c.call(&r.encode(), 5000).ok();
        match bytes.as_deref().and_then(Reply::decode) {
            Some(Reply::User(twime_proto::UserReply::Page { total, invalid_lines, .. })) => {
                println!("user_lexicon\twords={total}\tinvalid={invalid_lines}");
                if total != 1000 || invalid_lines != 0 {
                    std::process::exit(3);
                }
            }
            _ => {
                println!("user_lexicon\tunavailable");
                std::process::exit(3);
            }
        }
    }
    mem(pid, "memory_start");
    const S: u32 = 9;
    let press = |conn: &mut Conn, k: Key| conn.call(S, Event::Key(KeyEvent::press(k))).expect("key");
    let units = man["units"].as_array().expect("units");
    let (mut va, mut vb, mut vc, mut vd1, mut vd2) = (vec![], vec![], vec![], vec![], vec![]);
    let mut warm = 0usize;
    let mut seen = 0usize;
    for u in units.iter().cycle() {
        if warm >= 100 && seen >= units.len() {
            break;
        }
        let recording = warm >= 100;
        if recording {
            seen += 1;
        }
        let syl: Vec<String> = u["syllables"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        let k = u["pos"].as_u64().unwrap() as usize;
        let typed = u["typed"].as_str().unwrap().to_string();
        let repairs: Vec<String> = u["repairs"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        let front = u["front"].as_str().unwrap().to_string();
        conn.call(S, Event::SetContext(u["context"].as_str().unwrap().to_string())).expect("context");
        let mut t_a = None;
        for (i, x) in syl.iter().enumerate() {
            let x = if i == k { &typed } else { x };
            let keys = syllable_keys(x).expect("typable");
            for (j, key) in keys.iter().enumerate() {
                let t = Instant::now();
                press(&mut conn, *key);
                warm += 1;
                if i == k && j + 1 == keys.len() {
                    t_a = Some(t.elapsed().as_secs_f64() * 1000.0);
                }
            }
        }
        let seg = segments(&mut conn, S);
        let a_ok = seg.len() == syl.len() && seg[k].unconverted != auto && repairs.len() >= 2;
        for _ in k + 1..syl.len() {
            press(&mut conn, Key::Left);
        }
        let t = Instant::now();
        let u0 = press(&mut conn, Key::Down);
        let t_b = t.elapsed().as_secs_f64() * 1000.0;
        // the item to choose: auto = the first item of another repair reading than the first; suggest = the first item
        let other = |item: &str| repairs.iter().skip(usize::from(auto)).any(|g| item.ends_with(&format!("（{g}）")));
        let mut page_items = u0.candidates.clone();
        let mut pick = None;
        let mut pages = 0;
        while let Some(c) = page_items.clone() {
            if let Some(i) = c.items.iter().position(|it| other(it)) {
                pick = Some((i, c.items[i].chars().next().unwrap(), reading_of(&c.items[i])));
                break;
            }
            pages += 1;
            if pages > 40 {
                break;
            }
            let nu = press(&mut conn, Key::PageDown);
            if nu.candidates.as_ref().is_none_or(|x| x.page == 0) {
                break;
            }
            page_items = nu.candidates;
        }
        let b_ok = pick.is_some();
        let (mut t_c, mut chosen, mut reading) = (None, None, None);
        if let Some((i, ch, g)) = pick {
            let t = Instant::now();
            press(&mut conn, Key::Digit(i as u8 + 1));
            t_c = Some(t.elapsed().as_secs_f64() * 1000.0);
            chosen = Some(ch);
            reading = g;
        }
        // the reading in effect at a position: the reading shown by the current item of the list there (untimed)
        let reading_at = |conn: &mut Conn, j: usize| -> Option<String> {
            press(conn, Key::Home);
            for _ in 0..=j {
                press(conn, Key::Right);
            }
            let u = press(conn, Key::Down);
            let r = u.candidates.and_then(|c| c.items.first().and_then(|it| reading_of(it)));
            let _ = press(conn, Key::Esc);
            r
        };
        // before the choice: auto = the first repair reading, suggest = none (unconverted)
        let before = if auto { repairs.first().cloned() } else { None };
        let seg = segments(&mut conn, S);
        let c_ok = chosen.is_some() && reading.is_some() && reading != before && seg.len() == syl.len() && !seg[k].unconverted
            && seg[k].text.chars().next() == chosen && reading_at(&mut conn, k) == reading;
        press(&mut conn, Key::Home);
        let mut t_d1 = None;
        let keys = syllable_keys(&front).expect("typable");
        for (j, key) in keys.iter().enumerate() {
            let t = Instant::now();
            press(&mut conn, *key);
            if j + 1 == keys.len() {
                t_d1 = Some(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let seg = segments(&mut conn, S);
        let d1_ok = c_ok && seg.len() == syl.len() + 1 && seg[k + 1].text.chars().next() == chosen && reading_at(&mut conn, k + 1) == reading;
        // back to just after the front syllable
        press(&mut conn, Key::Home);
        press(&mut conn, Key::Right);
        let t = Instant::now();
        press(&mut conn, Key::Backspace);
        let t_d2 = t.elapsed().as_secs_f64() * 1000.0;
        let seg = segments(&mut conn, S);
        let d2_ok = c_ok && seg.len() == syl.len() && seg[k].text.chars().next() == chosen && reading_at(&mut conn, k) == reading;
        let _ = press(&mut conn, Key::Esc);
        let _ = press(&mut conn, Key::Esc);
        if !recording {
            continue;
        }
        if let (true, Some(x)) = (a_ok, t_a) {
            va.push(x);
        }
        if a_ok && b_ok {
            vb.push(t_b);
        }
        if let (true, Some(x)) = (c_ok, t_c) {
            vc.push(x);
        }
        if let (true, Some(x)) = (d1_ok, t_d1) {
            vd1.push(x);
        }
        if d2_ok {
            vd2.push(t_d2);
        }
    }
    let counts = [va.len(), vb.len(), vc.len(), vd1.len(), vd2.len()];
    line(&format!("repair_{mode}_{tag}_A_complete_typed"), va);
    line(&format!("repair_{mode}_{tag}_B_open"), vb);
    line(&format!("repair_{mode}_{tag}_C_choose_reading"), vc);
    line(&format!("repair_{mode}_{tag}_D1_front_insert"), vd1);
    line(&format!("repair_{mode}_{tag}_D2_front_backspace"), vd2);
    println!("repair_valid\tunits={}\tA={}\tB={}\tC={}\tD1={}\tD2={}", units.len(), counts[0], counts[1], counts[2], counts[3], counts[4]);
    mem(pid, "memory_end");
    if counts.iter().any(|&c| c < 500) {
        // plan 3.7: fewer than 500 valid observations = not acceptable (never skipped)
        std::process::exit(3);
    }
}

/// FUZZY F2 gate 6 (FUZZY-F2-plan 3.3), an engine started with `--repair suggest --fuzzy-words on|off`:
///   pnlat fuzzy <pipe> <lat-manifest.json> --tag on|off [--lexicon yes|no] [--cpu-mask HEX] [--engine-pid P]
/// Per manifest unit (key, context, syllables as typed, cursor): `SetContext`, every syllable typed, Left to the cursor,
/// Down (B, every opened list counts), and with `--tag on` PageDown to the first marked item and its number key (C). C
/// is valid when the composition's span equals the item text and, reopened at some position of the span, the list's
/// current item shows the chosen reading (it is in effect); checks are outside the timing. 100 warm-up keys first. Fewer than 500 valid B (or C with
/// `on`) = invalid round (exit 3).
fn fuzzy_lat(a: &[String]) {
    let pipe = &a[2];
    let man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).expect("manifest")).expect("manifest json");
    let tag = arg(a, "--tag").expect("--tag on|off");
    let on = match tag.as_str() {
        "on" => true,
        "off" => false,
        _ => panic!("--tag on|off"),
    };
    let lex = if arg(a, "--lexicon").as_deref() == Some("yes") { "lex" } else { "nolex" };
    let pid: Option<u32> = arg(a, "--engine-pid").and_then(|v| v.parse().ok());
    if let Some(m) = arg(a, "--cpu-mask") {
        unsafe {
            let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), usize::from_str_radix(&m, 16).unwrap());
        }
    }
    let mut conn = Conn { c: Client::connect(pipe, None, 200).expect("connect"), seq: 0 };
    println!("version\t{}", conn.hello().0);
    if lex == "lex" {
        conn.seq += 1;
        let r = Request { session: 0, seq: conn.seq, ack: None, event: Event::User(twime_proto::UserReq::List { query: String::new(), offset: 0, limit: 1 }) };
        let bytes = conn.c.call(&r.encode(), 5000).ok();
        match bytes.as_deref().and_then(Reply::decode) {
            Some(Reply::User(twime_proto::UserReply::Page { total, invalid_lines, .. })) => {
                println!("user_lexicon\twords={total}\tinvalid={invalid_lines}");
                if total != 1000 || invalid_lines != 0 {
                    std::process::exit(3);
                }
            }
            _ => {
                println!("user_lexicon\tunavailable");
                std::process::exit(3);
            }
        }
    }
    mem(pid, "memory_start");
    const S: u32 = 11;
    let press = |conn: &mut Conn, k: Key| conn.call(S, Event::Key(KeyEvent::press(k))).expect("key");
    let units = man["units"].as_array().expect("units");
    let (mut vb, mut vc) = (vec![], vec![]);
    let (mut warm, mut seen, mut with_fuzzy) = (0usize, 0usize, 0usize);
    for u in units.iter().cycle() {
        if warm >= 100 && seen >= units.len() {
            break;
        }
        let recording = warm >= 100;
        if recording {
            seen += 1;
        }
        let syl: Vec<String> = u["syllables"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        let e = u["cursor"].as_u64().unwrap() as usize;
        conn.call(S, Event::SetContext(u["context"].as_str().unwrap().to_string())).expect("context");
        for x in &syl {
            for key in syllable_keys(x).expect("typable") {
                press(&mut conn, key);
                warm += 1;
            }
        }
        for _ in e..syl.len() {
            press(&mut conn, Key::Left);
        }
        let t = Instant::now();
        let u0 = press(&mut conn, Key::Down);
        let t_b = t.elapsed().as_secs_f64() * 1000.0;
        let b_ok = u0.candidates.is_some();
        // the first marked item (a fuzzy suggestion: the manifest units have legal syllables only)
        let mut page = u0.candidates.clone();
        let mut pick = None;
        let mut pages = 0;
        while let Some(c) = page.clone() {
            if let Some(i) = c.items.iter().position(|it| it.ends_with('）') && it.contains('（')) {
                pick = Some((i, c.items[i].clone()));
                break;
            }
            pages += 1;
            if pages > 10 {
                break;
            }
            let nu = press(&mut conn, Key::PageDown);
            if nu.candidates.as_ref().is_none_or(|x| x.page == 0) {
                break;
            }
            page = nu.candidates;
        }
        let has = pick.is_some();
        let (mut t_c, mut c_ok) = (None, false);
        if let (true, Some((i, disp))) = (on, pick) {
            let t = Instant::now();
            press(&mut conn, Key::Digit(i as u8 + 1));
            t_c = Some(t.elapsed().as_secs_f64() * 1000.0);
            let text: String = disp.split('（').next().unwrap_or("").to_string();
            let l = text.chars().count();
            let seg = segments(&mut conn, S);
            let span_ok = seg.len() == syl.len() && l <= e && seg[e - l..e].iter().map(|g| g.text.as_str()).collect::<String>() == text;
            // the reading is in effect: at some position of the span the list's current item (an ordinary item, never a
            // suggestion) shows the reading g; without the change no ordinary item carries a reading here (legal input)
            let g = reading_of(&disp);
            let mut listed = false;
            for k in e - l.min(e)..e {
                press(&mut conn, Key::Home);
                for _ in 0..=k {
                    press(&mut conn, Key::Right);
                }
                let nu = press(&mut conn, Key::Down);
                let cur = nu.candidates.and_then(|c| c.items.first().cloned());
                let _ = press(&mut conn, Key::Esc);
                if g.is_some() && cur.as_deref().and_then(reading_of) == g {
                    listed = true;
                    break;
                }
            }
            c_ok = span_ok && listed;
        }
        let _ = press(&mut conn, Key::Esc);
        let _ = press(&mut conn, Key::Esc);
        let _ = press(&mut conn, Key::Esc);
        if !recording {
            continue;
        }
        with_fuzzy += usize::from(has);
        if b_ok {
            vb.push(t_b);
        }
        if let (true, Some(x)) = (c_ok, t_c) {
            vc.push(x);
        }
    }
    let (nb, nc) = (vb.len(), vc.len());
    line(&format!("fuzzy_{tag}_{lex}_B_open"), vb);
    if on {
        line(&format!("fuzzy_{tag}_{lex}_C_choose"), vc);
    }
    println!("fuzzy_valid\tunits={}\tB={nb}\tC={nc}\twith_fuzzy={with_fuzzy}", units.len());
    mem(pid, "memory_end");
    if nb < 500 || (on && nc < 500) {
        std::process::exit(3);
    }
}

///   pnlat fuzzyhint <pipe> <hint-manifest.json> --tag on|off [--cpu-mask HEX] [--engine-pid P]
/// FUZZY F2 step 2 (plan 5.3 S4), engines with `--fuzzy-hints on|off` (LEXUX and fuzzy words on in both). Per manifest
/// unit: `SetContext`, every syllable typed (T: the key completing each syllable, recorded in both tags), and with
/// `--tag on`: Left step by step (outside the timing) until the hint offers a fuzzy option, then `\` until the shown option
/// is the fuzzy one (W: that key). T counts when the finished composition shows a fuzzy hint (on) / for the same units
/// (off); W counts when the composition span shows the option text and, reopened inside the span, the list's current item
/// carries the corrected reading (checks outside the timing). 100 warm-up keys first. Fewer than 500 valid units (and W
/// with `on`) = invalid round (exit 3).
fn fuzzyhint_lat(a: &[String]) {
    let pipe = &a[2];
    let man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).expect("manifest")).expect("manifest json");
    let tag = arg(a, "--tag").expect("--tag on|off");
    let on = match tag.as_str() {
        "on" => true,
        "off" => false,
        _ => panic!("--tag on|off"),
    };
    let pid: Option<u32> = arg(a, "--engine-pid").and_then(|v| v.parse().ok());
    if let Some(m) = arg(a, "--cpu-mask") {
        unsafe {
            let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), usize::from_str_radix(&m, 16).unwrap());
        }
    }
    let mut conn = Conn { c: Client::connect(pipe, None, 200).expect("connect"), seq: 0 };
    println!("version\t{}", conn.hello().0);
    mem(pid, "memory_start");
    const S: u32 = 12;
    let press = |conn: &mut Conn, k: Key| conn.call(S, Event::Key(KeyEvent::press(k))).expect("key");
    let fuzzy_opt = |u: &Update| u.hint.as_ref().and_then(|h| h.options.iter().position(|o| o.ends_with('）') && o.contains('（')).map(|i| (i, h.current, h.options[i].clone())));
    let units = man["units"].as_array().expect("units");
    let (mut vt, mut vw) = (vec![], vec![]);
    let (mut warm, mut seen, mut valid_units) = (0usize, 0usize, 0usize);
    for u in units.iter().cycle() {
        if warm >= 100 && seen >= units.len() {
            break;
        }
        let recording = warm >= 100;
        if recording {
            seen += 1;
        }
        let syl: Vec<String> = u["syllables"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        conn.call(S, Event::SetContext(u["context"].as_str().unwrap().to_string())).expect("context");
        let mut times = vec![];
        let mut last = Update::default();
        for x in &syl {
            let keys = syllable_keys(x).expect("typable");
            for (i, key) in keys.iter().enumerate() {
                let t = Instant::now();
                last = press(&mut conn, *key);
                if i + 1 == keys.len() {
                    times.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                warm += 1;
            }
        }
        // the unit was chosen for a fuzzy hint at the end (dev half, on); off records the same units' typing
        let mut ok_t = !on || fuzzy_opt(&last).is_some();
        let (mut t_w, mut ok_w) = (None, false);
        if on && ok_t {
            let mut u1 = last.clone();
            let mut moved = 0;
            while u1.hint.as_ref().is_none_or(|_| fuzzy_opt(&u1).is_none()) && moved < syl.len() {
                u1 = press(&mut conn, Key::Left);
                moved += 1;
            }
            if let Some((idx, mut cur, disp)) = fuzzy_opt(&u1) {
                let mut presses = 0;
                while cur != idx && presses < 3 {
                    let t = Instant::now();
                    let nu = press(&mut conn, Key::Backslash);
                    t_w = Some(t.elapsed().as_secs_f64() * 1000.0);
                    cur = nu.hint.as_ref().map_or(usize::MAX, |h| h.current);
                    presses += 1;
                }
                let text: String = disp.split('（').next().unwrap_or("").to_string();
                let seg = segments(&mut conn, S);
                let joined: String = seg.iter().map(|g| g.text.as_str()).collect();
                let span_ok = cur == idx && joined.contains(&text);
                // the corrected reading is in effect: at some position the list's current item shows it
                let g = reading_of(&disp);
                let mut listed = false;
                for k in 0..syl.len() {
                    press(&mut conn, Key::Home);
                    for _ in 0..=k {
                        press(&mut conn, Key::Right);
                    }
                    let nu = press(&mut conn, Key::Down);
                    let c0 = nu.candidates.and_then(|c| c.items.first().cloned());
                    let _ = press(&mut conn, Key::Esc);
                    if g.is_some() && c0.as_deref().and_then(reading_of) == g {
                        listed = true;
                        break;
                    }
                }
                ok_w = span_ok && listed;
            } else {
                ok_t = false;
            }
        }
        for _ in 0..3 {
            let _ = press(&mut conn, Key::Esc);
        }
        if !recording || !ok_t {
            continue;
        }
        valid_units += 1;
        vt.extend(times);
        if let (true, Some(x)) = (ok_w, t_w) {
            vw.push(x);
        }
    }
    let nw = vw.len();
    line(&format!("fuzzyhint_{tag}_T_type"), vt);
    if on {
        line(&format!("fuzzyhint_{tag}_W_switch"), vw);
    }
    println!("fuzzyhint_valid\tunits={}\tvalid_units={valid_units}\tW={nw}", units.len());
    mem(pid, "memory_end");
    if valid_units < 500 || (on && nw < 500) {
        std::process::exit(3);
    }
}

///   pnlat aclist <pipe> <manifest.json> --tag on|off [--cpu-mask HEX] [--engine-pid P]
/// AUTOCOMP-LIST (plan 5.2 L4), engines with `--autocomp-list on|off`. Per manifest unit (key, context, syllables,
/// target word, its appended syllable count m): `SetContext`, every syllable typed, Down at the end (O: that key, both
/// tags); with `on`, PageDown to the target's page and its number key (S: that key). O counts when a list opened (and,
/// with `on`, the target is listed); S counts when the composition then ends with the target word and has m more
/// syllables (checked outside the timing). 100 warm-up keys first; fewer than 500 valid O (and S with `on`) = exit 3.
fn aclist_lat(a: &[String]) {
    let pipe = &a[2];
    let man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).expect("manifest")).expect("manifest json");
    let tag = arg(a, "--tag").expect("--tag on|off");
    let on = match tag.as_str() {
        "on" => true,
        "off" => false,
        _ => panic!("--tag on|off"),
    };
    let pid: Option<u32> = arg(a, "--engine-pid").and_then(|v| v.parse().ok());
    if let Some(m) = arg(a, "--cpu-mask") {
        unsafe {
            let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), usize::from_str_radix(&m, 16).unwrap());
        }
    }
    let mut conn = Conn { c: Client::connect(pipe, None, 200).expect("connect"), seq: 0 };
    println!("version\t{}", conn.hello().0);
    mem(pid, "memory_start");
    const S: u32 = 13;
    let press = |conn: &mut Conn, k: Key| conn.call(S, Event::Key(KeyEvent::press(k))).expect("key");
    let units = man["units"].as_array().expect("units");
    let (mut vo, mut vs) = (vec![], vec![]);
    let (mut warm, mut seen) = (0usize, 0usize);
    for u in units.iter().cycle() {
        if warm >= 100 && seen >= units.len() {
            break;
        }
        let recording = warm >= 100;
        if recording {
            seen += 1;
        }
        let syl: Vec<String> = u["syllables"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        let word = u["word"].as_str().unwrap().to_string();
        let m = u["m"].as_u64().unwrap() as usize;
        conn.call(S, Event::SetContext(u["context"].as_str().unwrap().to_string())).expect("context");
        for x in &syl {
            for key in syllable_keys(x).expect("typable") {
                press(&mut conn, key);
                warm += 1;
            }
        }
        let t = Instant::now();
        let u0 = press(&mut conn, Key::Down);
        let t_o = t.elapsed().as_secs_f64() * 1000.0;
        let mut ok_o = u0.candidates.is_some();
        let mut t_s = None;
        let mut ok_s = false;
        if on && ok_o {
            // find the target on its page, then its number key
            let mut page = u0.candidates.clone();
            let mut hit = None;
            for _ in 0..10 {
                let Some(c) = page.clone() else { break };
                if let Some(i) = c.items.iter().position(|it| *it == word) {
                    hit = Some(i);
                    break;
                }
                let nu = press(&mut conn, Key::PageDown);
                if nu.candidates.as_ref().is_none_or(|x| x.page == 0) {
                    break;
                }
                page = nu.candidates;
            }
            ok_o = hit.is_some();
            if let Some(i) = hit {
                let t = Instant::now();
                press(&mut conn, Key::Digit(i as u8 + 1));
                t_s = Some(t.elapsed().as_secs_f64() * 1000.0);
                let seg = segments(&mut conn, S);
                let text: String = seg.iter().map(|g| g.text.as_str()).collect();
                ok_s = seg.len() == syl.len() + m && text.ends_with(&word);
            }
        }
        for _ in 0..3 {
            let _ = press(&mut conn, Key::Esc);
        }
        if !recording {
            continue;
        }
        if ok_o {
            vo.push(t_o);
        }
        if let (true, Some(x)) = (ok_s, t_s) {
            vs.push(x);
        }
    }
    let (no, ns) = (vo.len(), vs.len());
    line(&format!("aclist_{tag}_O_open"), vo);
    if on {
        line(&format!("aclist_{tag}_S_select"), vs);
    }
    println!("aclist_valid\tunits={}\tO={no}\tS={ns}", units.len());
    mem(pid, "memory_end");
    if no < 500 || (on && ns < 500) {
        std::process::exit(3);
    }
}

///   pnlat emoji <pipe> <manifest.json> --tag on|off [--cpu-mask HEX] [--engine-pid P]
/// EMOJI (EMOJI-plan 5 V3), engines with `--emoji on|off` and an emoji.tsv. Per manifest unit (key, syllables, emoji
/// of its table row): `SetContext`, every syllable typed, Down at the end (O: that key, both tags); with `on`, End (the last
/// item is an emoji) and Enter (S: that key, a commit of exactly that last item). O counts when a list opened (with `on`: its
/// last item is one of the unit's emoji); S counts when the commit is that last item and the composition is gone (checked
/// outside the timing). 100 warm-up keys first, then the units cycle until 600 observations; fewer than 500 valid O (and S
/// with `on`) = exit 3.
fn emoji_lat(a: &[String]) {
    let pipe = &a[2];
    let man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).expect("manifest")).expect("manifest json");
    let tag = arg(a, "--tag").expect("--tag on|off");
    let on = match tag.as_str() {
        "on" => true,
        "off" => false,
        _ => panic!("--tag on|off"),
    };
    let pid: Option<u32> = arg(a, "--engine-pid").and_then(|v| v.parse().ok());
    if let Some(m) = arg(a, "--cpu-mask") {
        unsafe {
            let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), usize::from_str_radix(&m, 16).unwrap());
        }
    }
    let mut conn = Conn { c: Client::connect(pipe, None, 200).expect("connect"), seq: 0 };
    println!("version\t{}", conn.hello().0);
    mem(pid, "memory_start");
    const S: u32 = 14;
    const OBS: usize = 600;
    let press = |conn: &mut Conn, k: Key| conn.call(S, Event::Key(KeyEvent::press(k))).expect("key");
    let units = man["units"].as_array().expect("units");
    let (mut vo, mut vs) = (vec![], vec![]);
    let (mut warm, mut seen) = (0usize, 0usize);
    for u in units.iter().cycle() {
        if warm >= 100 && seen >= OBS {
            break;
        }
        let recording = warm >= 100;
        if recording {
            seen += 1;
        }
        let syl: Vec<String> = u["syllables"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        let emoji: Vec<&str> = u["emoji"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        conn.call(S, Event::SetContext(String::new())).expect("context");
        for x in &syl {
            for key in syllable_keys(x).expect("typable") {
                press(&mut conn, key);
                warm += 1;
            }
        }
        let t = Instant::now();
        let u0 = press(&mut conn, Key::Down);
        let t_o = t.elapsed().as_secs_f64() * 1000.0;
        let mut ok_o = u0.candidates.is_some();
        let (mut t_s, mut ok_s) = (None, false);
        if on && ok_o {
            // End: the last item (an emoji of the row); Enter: its choice
            let mut last = press(&mut conn, Key::End).candidates.and_then(|c| c.items.get(c.selected as usize).cloned());
            if !last.as_ref().is_some_and(|l| emoji.contains(&l.as_str())) {
                // ZHUYIN-OUT: the typed bopomofo is the very last item, the emoji end just before it
                last = press(&mut conn, Key::Up).candidates.and_then(|c| c.items.get(c.selected as usize).cloned());
            }
            ok_o = last.as_ref().is_some_and(|l| emoji.contains(&l.as_str()));
            if ok_o {
                let t = Instant::now();
                let ue = press(&mut conn, Key::Enter);
                t_s = Some(t.elapsed().as_secs_f64() * 1000.0);
                ok_s = ue.commit == last && ue.composition.is_none();
            }
        }
        for _ in 0..3 {
            let _ = press(&mut conn, Key::Esc);
        }
        if !recording {
            continue;
        }
        if ok_o {
            vo.push(t_o);
        }
        if let (true, Some(x)) = (ok_s, t_s) {
            vs.push(x);
        }
    }
    let (no, ns) = (vo.len(), vs.len());
    line(&format!("emoji_{tag}_O_open"), vo);
    if on {
        line(&format!("emoji_{tag}_S_select"), vs);
    }
    println!("emoji_valid\tunits={}\tO={no}\tS={ns}", units.len());
    mem(pid, "memory_end");
    if no < 500 || (on && ns < 500) {
        std::process::exit(3);
    }
}

///   pnlat zyout <pipe> <manifest.json> --tag base|new [--cpu-mask HEX] [--engine-pid P]
/// ZHUYIN-OUT (ZHUYIN-OUT-plan 7). Per manifest unit (`syllables`; at least 2): `SetContext`, all syllables but the
/// last typed, then C (the key completing the last syllable: the cost of one new syllable, the bar for S and M), O (Down at
/// the end: the list; with `new` its last item is the typed bopomofo), S (`new`: End, Enter: choosing the typed bopomofo at
/// the end, one new inference; timed: Enter), M (`new`: Esc, Left x k, Down, End, Enter at a position inside the text; k
/// from the unit's number; timed: Enter). O counts when a list opened (with `new`: its last item is the syllable);
/// S and M count when the commit is empty, the composition holds the syllable and the list is closed (checked outside the
/// timing). 100 warm-up keys first, then the units cycle until 600 observations; fewer than 500 valid C and O (and S, M with
/// `new`) = exit 3.
fn zyout_lat(a: &[String]) {
    let pipe = &a[2];
    let man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).expect("manifest")).expect("manifest json");
    let new = match arg(a, "--tag").expect("--tag base|new").as_str() {
        "new" => true,
        "base" => false,
        _ => panic!("--tag base|new"),
    };
    let pid: Option<u32> = arg(a, "--engine-pid").and_then(|v| v.parse().ok());
    if let Some(m) = arg(a, "--cpu-mask") {
        unsafe {
            let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), usize::from_str_radix(&m, 16).unwrap());
        }
    }
    let mut conn = Conn { c: Client::connect(pipe, None, 200).expect("connect"), seq: 0 };
    println!("version	{}", conn.hello().0);
    mem(pid, "memory_start");
    const S: u32 = 15;
    const OBS: usize = 600;
    let press = |conn: &mut Conn, k: Key| conn.call(S, Event::Key(KeyEvent::press(k))).expect("key");
    let units = man["units"].as_array().expect("units");
    let (mut vc, mut vo, mut vs, mut vm) = (vec![], vec![], vec![], vec![]);
    let (mut warm, mut seen) = (0usize, 0usize);
    for (ui, u) in units.iter().cycle().enumerate() {
        if warm >= 100 && seen >= OBS {
            break;
        }
        let recording = warm >= 100;
        if recording {
            seen += 1;
        }
        let syl: Vec<String> = u["syllables"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        if syl.len() < 2 {
            continue;
        }
        let n = syl.len();
        conn.call(S, Event::SetContext(String::new())).expect("context");
        for x in &syl[..n - 1] {
            for key in syllable_keys(x).expect("typable") {
                press(&mut conn, key);
                warm += 1;
            }
        }
        // C: the key that completes the last syllable
        let keys = syllable_keys(&syl[n - 1]).expect("typable");
        for key in &keys[..keys.len() - 1] {
            press(&mut conn, *key);
        }
        let t = Instant::now();
        let uc = press(&mut conn, keys[keys.len() - 1]);
        let t_c = t.elapsed().as_secs_f64() * 1000.0;
        let ok_c = uc.composition.as_ref().is_some_and(|c| c.segments.len() == n);
        // O: the list at the end
        let t = Instant::now();
        let uo = press(&mut conn, Key::Down);
        let t_o = t.elapsed().as_secs_f64() * 1000.0;
        let mut ok_o = uo.candidates.is_some();
        let (mut t_s, mut ok_s) = (None, false);
        if new && ok_o {
            let last = press(&mut conn, Key::End).candidates.and_then(|c| c.items.get(c.selected as usize).cloned());
            ok_o = last.as_deref() == Some(syl[n - 1].as_str());
            if ok_o {
                let t = Instant::now();
                let ue = press(&mut conn, Key::Enter);
                t_s = Some(t.elapsed().as_secs_f64() * 1000.0);
                ok_s = ue.commit.is_none() && ue.candidates.is_none() && ue.composition.as_ref().is_some_and(|c| c.segments.len() == n && c.segments[n - 1].text == syl[n - 1] && !c.segments[n - 1].unconverted && !c.segments[n - 1].locked);
            }
        }
        // M: a position inside the text
        let (mut t_m, mut ok_m) = (None, false);
        if new {
            for _ in 0..3 {
                let _ = press(&mut conn, Key::Esc);
            }
            conn.call(S, Event::SetContext(String::new())).expect("context");
            for x in &syl {
                for key in syllable_keys(x).expect("typable") {
                    press(&mut conn, key);
                }
            }
            let k = 1 + ui % (n - 1);
            for _ in 0..k {
                press(&mut conn, Key::Left);
            }
            let at = n - k - 1;
            let l = press(&mut conn, Key::Down);
            let last = l.candidates.and_then(|_| press(&mut conn, Key::End).candidates).and_then(|c| c.items.get(c.selected as usize).cloned());
            if last.as_deref() == Some(syl[at].as_str()) {
                let t = Instant::now();
                let ue = press(&mut conn, Key::Enter);
                t_m = Some(t.elapsed().as_secs_f64() * 1000.0);
                ok_m = ue.commit.is_none() && ue.candidates.is_none() && ue.composition.as_ref().is_some_and(|c| c.segments.len() == n && c.segments[at].text == syl[at] && !c.segments[at].unconverted && !c.segments[at].locked);
            }
        }
        for _ in 0..3 {
            let _ = press(&mut conn, Key::Esc);
        }
        if !recording {
            continue;
        }
        if ok_c {
            vc.push(t_c);
        }
        if ok_o {
            vo.push(t_o);
        }
        if let (true, Some(x)) = (ok_s, t_s) {
            vs.push(x);
        }
        if let (true, Some(x)) = (ok_m, t_m) {
            vm.push(x);
        }
    }
    let (nc, no, ns, nm) = (vc.len(), vo.len(), vs.len(), vm.len());
    line("zyout_C_complete", vc);
    line("zyout_O_open", vo);
    if new {
        line("zyout_S_select_end", vs);
        line("zyout_M_select_mid", vm);
    }
    println!("zyout_valid	units={}	C={nc}	O={no}	S={ns}	M={nm}", units.len());
    mem(pid, "memory_end");
    if nc < 500 || no < 500 || (new && (ns < 500 || nm < 500)) {
        std::process::exit(3);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.iter().any(|x|x=="--lexux-manifest") { std::panic::set_hook(Box::new(|_| eprintln!("measurement_errors=1"))); }
    match a.get(1).map(String::as_str) {
        Some("acpop") if a.len() >= 4 => pop::run(&a),
        Some("run") if a.len() >= 6 => run(&a),
        Some("repair") if a.len() >= 4 => repair(&a),
        Some("fuzzy") if a.len() >= 4 => fuzzy_lat(&a),
        Some("fuzzyhint") if a.len() >= 4 => fuzzyhint_lat(&a),
        Some("aclist") if a.len() >= 4 => aclist_lat(&a),
        Some("emoji") if a.len() >= 4 => emoji_lat(&a),
        Some("zyout") if a.len() >= 4 => zyout_lat(&a),
        _ => {
            eprintln!("usage: see the header of pnlat.rs");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn document_scenarios_follow_capability_on_version_five() {
        for version in 1..=4 {
            assert!(hello_info(Reply::Hello { version }) == Some((version, version == 4)));
        }
        assert!(hello_info(Reply::HelloCaps { version: 5, caps: 0 }) == Some((5, false)));
        assert!(hello_info(Reply::HelloCaps { version: 5, caps: 2 }) == Some((5, false)));
        assert!(hello_info(Reply::HelloCaps { version: 5, caps: twime_proto::CAP_DOC_TEXT }) == Some((5, true)));
        assert!(hello_info(Reply::HelloCaps { version: 5, caps: 3 }) == Some((5, true)));
    }
}

#[derive(serde::Deserialize)]
struct LexuxMeasureCase {
    context: String,
    syllables: Vec<String>,
    #[serde(default)] options: Vec<String>,
    #[serde(default)] start: usize,
    #[serde(default)] len: usize,
    #[serde(default)] cursor: Option<usize>,
}
#[derive(serde::Deserialize)]
struct LexuxMeasure {
    lexdense: Vec<LexuxMeasureCase>,
    swap: Vec<LexuxMeasureCase>,
}
fn lexux_warmup(conn: &mut Conn, syllables: &[String]) {
    let mut n=0;
    conn.call(90, Event::SetContext(String::new())).expect("warmup reset");
    for s in syllables.iter().cycle() {
        for key in syllable_keys(s).expect("warmup keys") {
            conn.call(90,Event::Key(KeyEvent::press(key))).expect("warmup key"); n+=1;
            if n==100 { break }
        }
        if n==100 { break }
    }
    assert_eq!(n,100);
    conn.call(90,Event::Key(KeyEvent::press(Key::Esc))).expect("warmup cancel");
}
fn lexux_measure(conn: &mut Conn, a: &[String], n: usize) {
    let Some(path)=arg(a,"--lexux-manifest") else {return};
    let m: LexuxMeasure=serde_json::from_slice(&std::fs::read(path).expect("measurement manifest")).expect("measurement json");
    assert_eq!(m.lexdense.len(),20); assert!(m.swap.len()>=100);
    let enabled=a.iter().any(|x|x=="--lexux-on");
    let(mut trig,mut other)=(vec![],vec![]);
    for k in 0..n {
        let c=&m.lexdense[k % m.lexdense.len()];
        assert_eq!(c.syllables.len(),39);
        conn.call(91,Event::SetContext(c.context.clone())).expect("lexdense reset");
        assert!(type_syl(conn,91,&c.syllables,&mut trig,&mut other));
        conn.call(91,Event::Key(KeyEvent::press(Key::Esc))).expect("lexdense cancel");
    }
    line("lexdense_trigger",trig);line("lexdense_other",other);
    let mut swaps=vec![]; let mut assertions=0;
    for k in 0..n {
        let c=&m.swap[k % m.swap.len()];
        conn.call(92,Event::SetContext(c.context.clone())).expect("swap reset");
        assert!(type_syl(conn,92,&c.syllables,&mut vec![],&mut vec![]));
        if let Some(cursor)=c.cursor {
            conn.call(92,Event::Key(KeyEvent::press(Key::Home))).expect("swap cursor home");
            for _ in 0..cursor {conn.call(92,Event::Key(KeyEvent::press(Key::Right))).expect("swap cursor");}
        }
        let before=conn.call(92,Event::Ping).expect("swap state");
        let baseline=before.composition.as_ref().expect("swap composition").segments.iter().map(|g|g.text.as_str()).collect::<String>();
        if enabled {
            let hint=before.hint.expect("active swap hint");
            assert_eq!(hint.current,0);assert_eq!(hint.options,c.options);
        } else {assert!(before.hint.is_none())}
        assert!(c.options.len()>=2);
        for i in 1..=c.options.len() {
            let t=Instant::now();
            let u=conn.call(92,Event::Key(KeyEvent::press(Key::Backslash))).expect("swap key");
            swaps.push(t.elapsed().as_secs_f64()*1000.);
            if enabled {
                assert!(u.commit.is_none());
                let h=u.hint.expect("swap record");
                assert_eq!(h.current,i % c.options.len());assert_eq!(h.options,c.options);
                let seg=u.composition.expect("swap update").segments;
                assert!(seg[c.start..c.start+c.len].iter().all(|s|s.locked));
                if i==c.options.len() {assert_eq!(seg.iter().map(|g|g.text.as_str()).collect::<String>(),baseline)}
            } else if i==1 {
                assert_eq!(u.commit.as_deref(),Some(format!("{baseline}\\").as_str()));
            }
            assertions+=1;
        }
        conn.call(92,Event::Key(KeyEvent::press(Key::Esc))).expect("swap cancel");
    }
    line("swap",swaps);println!("swap_assertions={assertions} enabled={}",usize::from(enabled));
}
