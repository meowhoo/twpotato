//! EMOJI (EMOJI-plan 5 V2): off / on equivalence against a frozen pre-construction snapshot.
//!
//!   fuzzf2 emoji <settings> <units.tsv> <out.jsonl> <threads> [emoji.tsv]
//!   fuzzf2 emoji-keys <settings> <emoji.tsv>      representative keywords: lookup, display at the end of the list, choice
//!
//! units.tsv: key, context, syllables [, gold]. Per unit: `off_fp` = a hash of every typing Update (protocol v1-v7 bytes), of
//! the list opened after each syllable (bytes and full item identity) and of the composition after choosing the second item.
//! The pre-construction build prints the same fingerprint (its shim has no table), so the two outputs can be diffed. With a
//! table the on engine is compared with the off one: typing and the Enter commit identical (S1), the list without its emoji
//! equal to the off list (S2), the first item and the choice of the second item identical (S3); the emoji items are chosen
//! with Enter and must commit exactly their text (S4).
use super::emoji_shim::{raw_flags, with_table};
use super::zyout_shim::lit_flags;
use super::*;
use twime_proto::Reply;

fn bytes(u: &Update, h: &mut DefaultHasher) {
    for v in 1..=7 {
        Reply::Update { session: 1, seq: 1, update: u.clone() }.encode_for(v).hash(h);
    }
}

type Ids = Vec<(String, usize, Vec<(usize, String)>, Vec<String>)>;

fn engines3(m: &HashMap<String, String>, assets: &Arc<Assets>, table: Option<&str>) -> (Arc<Engine>, Option<Arc<Engine>>) {
    let lambda: f64 = m.get("lambda").map_or(0.5, |x| x.parse().unwrap());
    let data = fs::read_to_string(&m["data_txt"]).expect("data.txt");
    let index = Arc::new(fuzzy::Index::from_data_txt(assets, &data));
    // the product's list features: F1 suggest, F2 words, AUTOCOMP-LIST
    let base = Config {
        lambda,
        candidates: true,
        repair: Some(twime_core::Repair { mode: twime_core::RepairMode::Suggest, index: index.clone() }),
        fuzzy_words: Some(index),
        autocomp_list: true,
        ..Config::default()
    };
    let on = table.map(|t| Engine::new(assets.clone(), with_table(base.clone(), t)));
    (Engine::new(assets.clone(), base), on)
}

/// `EMOJI_UX=1`: sessions of a version-3 shell (`set_ux`, the product path with the punctuation / Shift-letter handling); the
/// default is the version-1/2 key handling. Both builds (snapshot and new) read it the same way.
fn ux() -> bool {
    std::env::var("EMOJI_UX").is_ok_and(|v| v == "1")
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

/// The session after typing `syl[..n]` and opening the list.
fn listed(e: &Arc<Engine>, ctx: &str, syl: &[String], n: usize) -> (Session, Update) {
    let mut s = fresh(e, ctx);
    for x in &syl[..n] {
        type_syl(&mut s, x);
    }
    let u = press(&mut s, Key::Down);
    (s, u)
}

fn strip(ids: Ids, raw: &[bool]) -> Ids {
    ids.into_iter().enumerate().filter(|(i, _)| !raw.get(*i).copied().unwrap_or(false)).map(|(_, x)| x).collect()
}

pub(super) fn run(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let (off_e, on_e) = engines3(&m, &assets, a.get(6).map(String::as_str));
    let out = par(rows(&a[3]), a[5].parse().unwrap(), move |r| {
        let syl = syls(&r[2]);
        let n = syl.len();
        let mut h = DefaultHasher::new();
        // S1 / off fingerprint: typing without a list
        let mut off = fresh(&off_e, &r[1]);
        let mut on = on_e.as_ref().map(|e| fresh(e, &r[1]));
        let mut same_typing = true;
        for x in &syl {
            for k in keys_of(x) {
                let uo = press(&mut off, k);
                bytes(&uo, &mut h);
                if let Some(on) = on.as_mut() {
                    let un = press(on, k);
                    same_typing &= format!("{uo:?}") == format!("{un:?}");
                }
            }
        }
        let eo = press(&mut off, Key::Enter);
        bytes(&eo, &mut h);
        if let Some(on) = on.as_mut() {
            same_typing &= format!("{eo:?}") == format!("{:?}", press(on, Key::Enter));
        }
        // S2 / S3 / S4: the list after each syllable
        let (mut lists, mut with_emoji, mut strip_ok, mut first_ok, mut sel_ok, mut pick_ok, mut picks) = (0usize, 0usize, true, true, true, true, 0usize);
        let (mut tail_ok, mut hon) = (true, DefaultHasher::new());
        for i in 1..=n {
            let (so, uo) = listed(&off_e, &r[1], &syl, i);
            bytes(&uo, &mut h);
            let ids_off = so.candidate_identities();
            ids_off.hash(&mut h);
            lists += usize::from(!ids_off.is_empty());
            if i == n || i == 1 {
                // choosing the second item
                let mut s2 = so;
                let u2 = press(&mut s2, Key::Digit(2));
                bytes(&u2, &mut h);
                format!("{:?}", s2.candidate_items()).hash(&mut h);
            }
            let Some(e) = on_e.as_ref() else { continue };
            let (mut sn, un) = listed(e, &r[1], &syl, i);
            let flags = raw_flags(&sn);
            let ids_on = sn.candidate_identities();
            strip_ok &= strip(ids_on.clone(), &flags) == ids_off;
            // the emoji are one block at the very end; the page shown is the off page when no emoji reach it
            let first_raw = flags.iter().position(|f| *f);
            // ZHUYIN-OUT: the typed bopomofo, when there, is the very last item after the emoji block
            let end = flags.len() - usize::from(lit_flags(&sn).last() == Some(&true));
            tail_ok &= flags.len() == ids_on.len() && first_raw.is_none_or(|i| flags[i..end].iter().all(|f| *f));
            (&ids_on, &flags).hash(&mut hon);
            if let (Some(co), Some(cn)) = (&uo.candidates, &un.candidates) {
                first_ok &= co.items.first() == cn.items.first() && co.selected == cn.selected && uo.committable == un.committable && uo.composition == un.composition;
            } else {
                first_ok &= uo.candidates.is_some() == un.candidates.is_some();
            }
            if flags.iter().any(|f| *f) {
                with_emoji += 1;
                // choosing the second item (when it is an ordinary one) gives the same composition as off
                if flags.get(1) == Some(&false) {
                    let (mut so2, _) = listed(&off_e, &r[1], &syl, i);
                    let (uo2, un2) = (press(&mut so2, Key::Digit(2)), press(&mut sn, Key::Digit(2)));
                    sel_ok &= uo2.composition == un2.composition && uo2.committable == un2.committable && uo2.commit == un2.commit;
                }
                // the same ordinary item (by identity: the last one before the emoji) chosen with Enter in both lists: composition (text and
                // cursor), committability and commit equal
                if let Some(k) = first_raw.and_then(|f| f.checked_sub(1)) {
                    let ((mut sa, _), (mut sb, _)) = (listed(&off_e, &r[1], &syl, i), listed(e, &r[1], &syl, i));
                    for _ in 0..k {
                        press(&mut sa, Key::Down);
                        press(&mut sb, Key::Down);
                    }
                    let (ua, ub) = (press(&mut sa, Key::Enter), press(&mut sb, Key::Enter));
                    sel_ok &= ids_off[k] == ids_on[k] && ua.composition == ub.composition && ua.committable == ub.committable && ua.commit == ub.commit;
                }
                // every emoji item, reached with Down and chosen with Enter, is shown as its text and commits exactly that
                for (j, f) in flags.iter().enumerate() {
                    if !*f {
                        continue;
                    }
                    let (mut sp, mut up) = listed(e, &r[1], &syl, i);
                    for _ in 0..j {
                        up = press(&mut sp, Key::Down);
                    }
                    let shown = up.candidates.as_ref().and_then(|c| c.items.get(c.selected as usize).cloned());
                    let up = press(&mut sp, Key::Enter);
                    pick_ok &= shown.as_deref() == Some(ids_on[j].0.as_str())
                        && up.commit.as_deref() == Some(ids_on[j].0.as_str())
                        && up.composition.is_none()
                        && !up.committable
                        && up.candidates.is_none();
                    picks += 1;
                }
            }
        }
        let fp = h.finish();
        let mut o = serde_json::json!({"key": r[0], "n": n, "off_fp": format!("{fp:016x}"), "lists": lists});
        if on_e.is_some() {
            o["same_typing"] = same_typing.into();
            o["strip_ok"] = strip_ok.into();
            o["tail_ok"] = tail_ok.into();
            o["on_fp"] = format!("{:016x}", hon.finish()).into();
            o["first_ok"] = first_ok.into();
            o["sel_ok"] = sel_ok.into();
            o["pick_ok"] = pick_ok.into();
            o["with_emoji"] = with_emoji.into();
            o["picks"] = picks.into();
        }
        o.to_string()
    });
    write(&a[4], &out);
    println!("emoji rows {}", out.len());
}

/// Representative keywords with expected emoji (the nonzero gate): lookup, display at the end of the list, choice.
pub(super) fn keys(a: &[String]) {
    let m = settings(&a[2]);
    let assets = Arc::new(load(&m));
    let (_, on) = engines3(&m, &assets, Some(&a[3]));
    let on = on.unwrap();
    let cases: [(&str, &str, &[&str]); 6] = [
        ("愛心", "ㄞˋ ㄒㄧㄣ", &["❤\u{fe0f}"]),
        ("笑臉", "ㄒㄧㄠˋ ㄌㄧㄢˇ", &["😀", "😊"]),
        ("貓", "ㄇㄠ", &["🐈", "🐱"]),
        ("微笑", "ㄨㄟ ㄒㄧㄠˋ", &["🙂", "😊"]),
        ("月亮", "ㄩㄝˋ ㄌㄧㄤˋ", &["🌙"]),
        ("火", "ㄏㄨㄛˇ", &["🔥"]),
    ];
    let mut fail = 0;
    for (kw, rd, want) in cases {
        let syl = syls(rd);
        let (mut s, u) = listed(&on, "", &syl, syl.len());
        let items = s.candidate_items().unwrap_or_default();
        let flags = raw_flags(&s);
        // the emoji are the tail of the list; the page shown holds the first items
        let first_raw = flags.iter().position(|f| *f);
        let end = flags.len() - usize::from(lit_flags(&s).last() == Some(&true));
        let tail_ok = first_raw.is_some_and(|i| flags[i..end].iter().all(|f| *f));
        let hit = first_raw.and_then(|i| items[i..].iter().position(|it| want.contains(&it.0.as_str())).map(|j| i + j));
        let mut ok = tail_ok && hit.is_some() && u.candidates.as_ref().is_some_and(|c| c.total as usize == items.len());
        let mut got = None;
        if let Some(j) = hit {
            // the page the user sees (Cands.items) shows the emoji as the highlighted item, and Enter commits it
            let mut shown = u.clone();
            for _ in 0..j {
                shown = press(&mut s, Key::Down);
            }
            let shown_item = shown.candidates.as_ref().and_then(|c| c.items.get(c.selected as usize).cloned());
            let up = press(&mut s, Key::Enter);
            got = up.commit.clone();
            ok &= shown_item.as_deref() == Some(items[j].0.as_str()) && up.commit.as_deref() == Some(items[j].0.as_str()) && up.composition.is_none();
        }
        println!("{} {kw}: list {} items, emoji {}, commit {:?}", if ok { "PASS" } else { "FAIL" }, items.len(), flags.iter().filter(|f| **f).count(), got);
        fail += usize::from(!ok);
    }
    if fail > 0 {
        std::process::exit(1);
    }
}
