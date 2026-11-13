//! ZHUYIN-OUT (ZHUYIN-OUT-plan 6.2): equivalence of the build with the feature against a frozen pre-construction
//! snapshot, G1-G3, and the feature's own checks.
//!
//!   fuzzf2 zyout <settings> <units.tsv> <out.jsonl> <threads> [emoji.tsv]
//!
//! units.tsv: key, context, syllables [, gold]. Per unit three fingerprints (`g1`, `g2`, `g3`), equal in the two builds
//! for every unit (the pre-construction build prints the same fingerprints, its shim reports no typed-bopomofo item):
//! * g1: typing without a list: the protocol bytes (v1-v7) of every key and of Enter, of Shift+Enter (the raw-key rescue)
//!   and of an editing script (Left, Delete, Backspace, Right, End, Home, Esc);
//! * g2: the list opened after each syllable (and with the cursor left of the end): the open flag, the items without the
//!   typed bopomofo (full identity), the first page and the total without it, and everything else of the Update;
//! * g3: choosing the first, the second and the last ordinary item, closing the list with Esc and Backspace, and typing on
//!   to Enter: the protocol bytes after each key.
//! The feature's own checks (zero in the snapshot): `lit_lists` lists with the typed bopomofo, `g4a` lists holding only it,
//! `lit_fail` = last-item / choose / commit failures, `leak` = internal marks seen in any Update string.
use super::zyout_shim::lit_flags;
use super::*;
use twime_proto::{Mods, Reply};

fn bytes(u: &Update, h: &mut DefaultHasher) {
    for v in 1..=7 {
        Reply::Update { session: 1, seq: 1, update: u.clone() }.encode_for(v).hash(h);
    }
}

/// Everything of an Update but the list (and the timing).
fn nolist(u: &Update) -> String {
    format!(
        "{:?}{:?}",
        (&u.handled, &u.commit, &u.composition, &u.composing, &u.committable, &u.mode, &u.punct_half),
        (&u.run, &u.symbols, &u.hint, &u.completion, &u.pop_gen, &u.notice, &u.toggle_key)
    )
}

type Ids = Vec<(String, usize, Vec<(usize, String)>, Vec<String>)>;

fn engine(m: &HashMap<String, String>, assets: &Arc<Assets>, table: Option<&str>) -> Arc<Engine> {
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let data = fs::read_to_string(&m["data_txt"]).expect("data.txt");
    let index = Arc::new(fuzzy::Index::from_data_txt(assets, &data));
    // the product's list features: F1 suggest, F2 words, AUTOCOMP-LIST (and EMOJI when a table is given)
    let base = Config {
        lambda,
        candidates: true,
        repair: Some(twime_core::Repair { mode: twime_core::RepairMode::Suggest, index: index.clone() }),
        fuzzy_words: Some(index),
        autocomp_list: true,
        ..Config::default()
    };
    let cfg = match table {
        Some(t) => super::emoji_shim::with_table(base, t),
        None => base,
    };
    Engine::new(assets.clone(), cfg)
}

/// `ZY_UX=1`: sessions of a version-3 shell (the product path); default the version-1/2 key handling.
fn ux() -> bool {
    std::env::var("ZY_UX").is_ok_and(|v| v == "1")
}

fn fresh(e: &Arc<Engine>, ctx: &str) -> Session {
    let mut s = e.new_session();
    s.set_ux(ux());
    s.handle(1, None, Event::SetContext(ctx.to_string()));
    s
}

fn type_syl(s: &mut Session, x: &str) {
    for k in keys_of(x) {
        press(s, k);
    }
}

fn shift_enter(s: &mut Session) -> Update {
    s.on_key(KeyEvent { key: Key::Enter, mods: Mods(Mods::SHIFT), down: true })
}

#[derive(Default)]
struct Stats {
    lists: usize,
    lit_lists: usize,
    g4a: usize,
    lit_fail: usize,
    leak: usize,
    lit_pick: usize,
}

impl Stats {
    fn scan(&mut self, u: &Update) {
        let mut strings: Vec<String> = vec![];
        strings.extend(u.commit.clone());
        strings.extend(u.composition.iter().map(|c| c.text()));
        strings.extend(u.candidates.iter().flat_map(|c| c.items.clone()));
        self.leak += strings.iter().filter(|t| t.contains('\u{E000}') || t.contains('\u{02C9}')).count();
    }
}

/// The session after typing `syl[..i]`, `left` Left keys and Down; the Update of Down.
fn listed(e: &Arc<Engine>, ctx: &str, syl: &[String], i: usize, left: usize) -> (Session, Update) {
    let mut s = fresh(e, ctx);
    for x in &syl[..i] {
        type_syl(&mut s, x);
    }
    for _ in 0..left {
        press(&mut s, Key::Left);
    }
    let u = press(&mut s, Key::Down);
    (s, u)
}

fn strip(ids: Ids, flags: &[bool]) -> Ids {
    ids.into_iter().enumerate().filter(|(i, _)| !flags.get(*i).copied().unwrap_or(false)).map(|(_, x)| x).collect()
}

/// G2 for the list at (i, left); returns the number of ordinary items.
fn g2(e: &Arc<Engine>, ctx: &str, syl: &[String], i: usize, left: usize, h: &mut DefaultHasher, st: &mut Stats) -> usize {
    let (s, u) = listed(e, ctx, syl, i, left);
    st.scan(&u);
    let flags = lit_flags(&s);
    let ids = s.candidate_identities();
    let stripped = strip(ids.clone(), &flags);
    let has_lit = flags.iter().any(|f| *f);
    st.lists += usize::from(!ids.is_empty());
    st.lit_lists += usize::from(has_lit);
    st.g4a += usize::from(has_lit && stripped.is_empty());
    // the typed bopomofo is one item and the last one
    if has_lit && !(flags.iter().filter(|f| **f).count() == 1 && flags.last() == Some(&true)) {
        st.lit_fail += 1;
    }
    nolist(&u).hash(h);
    (!stripped.is_empty()).hash(h);
    stripped.hash(h);
    if !stripped.is_empty() {
        // an ordinary list is open (the open flag is the list's presence in the Update)
        let c = u.candidates.as_ref().filter(|_| u.candidates_open).expect("an ordinary list is open");
        let ps = e.config().page_size.max(1);
        let lit_total = usize::from(has_lit);
        let mut page0 = c.items.clone();
        if c.page == 0 && has_lit && c.total as usize <= ps {
            page0.pop(); // the typed bopomofo on the first page
        }
        (c.page, c.selected, c.per_page, c.total as usize - lit_total).hash(h);
        if c.page == 0 {
            page0.hash(h);
        }
    }
    stripped.len()
}

/// Continues with the rest of the unit and Enter.
fn finish(s: &mut Session, syl: &[String], from: usize, h: &mut DefaultHasher, st: &mut Stats) {
    for x in &syl[from..] {
        for k in keys_of(x) {
            let u = press(s, k);
            st.scan(&u);
            bytes(&u, h);
        }
    }
    let u = press(s, Key::Enter);
    st.scan(&u);
    bytes(&u, h);
}

/// G3 for the list at (i, left): the choices and the closers.
fn g3(e: &Arc<Engine>, ctx: &str, syl: &[String], i: usize, left: usize, n_ord: usize, h: &mut DefaultHasher, st: &mut Stats) {
    let mut ks = vec![];
    if n_ord >= 1 {
        ks.push(0usize);
    }
    if n_ord >= 2 {
        ks.push(1);
    }
    if (3..=40).contains(&n_ord) {
        ks.push(n_ord - 1);
    }
    for k in ks {
        let (mut s, _) = listed(e, ctx, syl, i, left);
        for _ in 0..k {
            press(&mut s, Key::Down);
        }
        let u = press(&mut s, Key::Enter);
        st.scan(&u);
        bytes(&u, h);
        if left == 0 {
            finish(&mut s, syl, i, h, st);
        }
    }
    if n_ord >= 1 {
        for closer in [Key::Esc, Key::Backspace] {
            let (mut s, _) = listed(e, ctx, syl, i, left);
            let u = press(&mut s, closer);
            st.scan(&u);
            bytes(&u, h);
        }
    }
    // the typed bopomofo itself: choosing it keeps what is shown equal to what is committed
    let (mut s, _) = listed(e, ctx, syl, i, left);
    if lit_flags(&s).last() == Some(&true) {
        press(&mut s, Key::End);
        let u = press(&mut s, Key::Enter);
        st.scan(&u);
        st.lit_pick += 1;
        let shown = u.composition.as_ref().map(|c| c.text());
        // the chosen position (the one before the cursor of the list) now shows its typed syllable, as plain text
        let pos = i - 1 - left;
        let seg_ok = u.composition.as_ref().is_some_and(|c| {
            c.segments.get(pos).is_some_and(|g| g.text == syl[pos] && !g.pending && !g.unconverted && !g.locked && g.hint == 0)
        });
        let ok = seg_ok && u.commit.is_none() && u.candidates.is_none() && !u.candidates_open && u.composition.is_some() && u.committable;
        let c = press(&mut s, Key::Enter);
        st.scan(&c);
        if !(ok && c.commit == shown && c.composition.is_none()) {
            st.lit_fail += 1;
        }
    }
}

pub(super) fn run(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let e = engine(&m, &assets, a.get(6).map(String::as_str));
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let (ctx, syl) = (r[1].as_str(), syls(&r[2]));
        let n = syl.len();
        let (mut h1, mut h2, mut h3) = (DefaultHasher::new(), DefaultHasher::new(), DefaultHasher::new());
        let mut st = Stats::default();
        // G1: typing, then Enter / the raw-key rescue / an editing script
        for script in 0..3 {
            let mut s = fresh(&e, ctx);
            let upto = if script == 2 { n.min(4) } else { n };
            for x in &syl[..upto] {
                for k in keys_of(x) {
                    let u = press(&mut s, k);
                    st.scan(&u);
                    bytes(&u, &mut h1);
                }
            }
            let tail: Vec<Update> = match script {
                0 => vec![press(&mut s, Key::Enter)],
                1 => vec![shift_enter(&mut s)],
                _ => [Key::Left, Key::Delete, Key::Backspace, Key::Right, Key::End, Key::Home, Key::Esc].into_iter().map(|k| press(&mut s, k)).collect(),
            };
            for u in &tail {
                st.scan(u);
                bytes(u, &mut h1);
            }
        }
        // G2 after each syllable, G3 at the first, the middle and the last
        let picks = [1, n.div_ceil(2), n];
        for i in 1..=n {
            let n_ord = g2(&e, ctx, &syl, i, 0, &mut h2, &mut st);
            if picks.contains(&i) {
                g3(&e, ctx, &syl, i, 0, n_ord, &mut h3, &mut st);
            }
        }
        // with the cursor left of the end
        for left in [1, n / 2] {
            if left >= 1 && left < n {
                let n_ord = g2(&e, ctx, &syl, n, left, &mut h2, &mut st);
                g3(&e, ctx, &syl, n, left, n_ord, &mut h3, &mut st);
            }
        }
        serde_json::json!({
            "key": r[0], "n": n,
            "g1": format!("{:016x}", h1.finish()), "g2": format!("{:016x}", h2.finish()), "g3": format!("{:016x}", h3.finish()),
            "lists": st.lists, "lit_lists": st.lit_lists, "g4a": st.g4a, "lit_pick": st.lit_pick, "lit_fail": st.lit_fail, "leak": st.leak,
        })
        .to_string()
    });
    write(&a[4], &out);
    println!("zyout rows {}", out.len());
}
