//! SHORT-PRIOR stage A (SHORT-PRIOR-plan 1): F3 context tail, WB word jumps, settings off = old path.

use super::*;
use crate::prior::ShortCfg;

const DATA: &str = "ㄌㄧˋ 立 -3\nㄌㄧˋ 例 -3.5\nㄌㄧˋ 力 -3.2\nㄐㄩˋ 劇 -3\nㄐㄩˋ 句 -3.4\nㄐㄩˋ 具 -3.3\n\
ㄌㄧˋ-ㄐㄩˋ 例句 -5.87\nㄌㄧˋ-ㄐㄩˋ 力具 -7.47\nㄓㄠˇ 找 -3\n";

fn assets() -> Assets {
    let mut a = Assets::from_dictionary_text(DATA, "").unwrap();
    a.model = Some(crate::n1::n1_prefix_tests::model(a.vocab.len()));
    a
}

fn syl(s: &str) -> Vec<String> {
    s.split(' ').map(String::from).collect()
}

fn text(c: &Conversion) -> String {
    c.chars.iter().map(|x| x.unwrap_or('?')).collect()
}

#[test]
fn context_length_and_f3_tail() {
    let a = assets();
    let s = syl("ㄌㄧˋ ㄐㄩˋ");
    let c = a.convert("", &s, &[], 0.0);
    assert_eq!((c.inf.ctx_len, c.inf.ctx_tail.clone()), (0, vec![]));
    let c = a.convert("找例句", &s, &[], 0.0);
    let id = |ch| a.vocab.char_id(ch);
    assert_eq!((c.inf.ctx_len, c.inf.ctx_tail.clone()), (3, vec![id('找'), id('例'), id('句')]));
    // an out-of-vocabulary character (punctuation) stops the tail; d counts every context character the model reads
    let c = a.convert("立，找", &s, &[], 0.0);
    assert_eq!((c.inf.ctx_len, c.inf.ctx_tail.clone()), (3, vec![id('找')]));
    let long: String = std::iter::repeat_n('找', 80).collect();
    assert_eq!(a.convert(&long, &s, &[], 0.0).inf.ctx_len, crate::n1::CONTEXT);
    // the F3 view only carries the tail when prior_ctx is on
    assert!(a.short_view(&c.inf).pre().is_empty());
}

#[test]
fn word_jumps_follow_alpha_floor_and_table() {
    let mut a = assets();
    let s = syl("ㄌㄧˋ ㄐㄩˋ");
    let base = a.convert("", &s, &[], 0.0);
    let off = text(&base);
    let cfg = |c, wb| ShortCfg { c, l0: None, wb, ctx: false };
    a.set_short(cfg(4, 0.0)).unwrap();
    assert_eq!(text(&a.rebeam(&base, &[], 0.0)), off, "beta 0: no jumps, the old path");
    a.set_short(cfg(4, 50.0)).unwrap();
    assert_eq!(text(&a.rebeam(&base, &[], 0.0)), "例句", "a word above the floor wins with a large bonus");
    // the floor word (log10 = the minimum of its length) never gets a T1 jump
    let lp = crate::pn::log_softmax(&base.inf.scores);
    let sv = a.short_view(&base.inf);
    let j = a.short_word_jumps(&s, &base.inf.sets, &lp, &[None, None], &[false, false], &sv);
    assert_eq!(j.len(), 1);
    assert_eq!((j[0].start, j[0].bonus), (0, 50.0));
    // d >= C: alpha 0, no jump, the old output
    let ctx = a.convert("找找找找", &s, &[], 0.0);
    a.set_short(ShortCfg::default()).unwrap();
    let ctx_off = text(&a.rebeam(&ctx, &[], 0.0));
    a.set_short(cfg(4, 50.0)).unwrap();
    assert_eq!(text(&a.rebeam(&ctx, &[], 0.0)), ctx_off);
    // T2 overrides the floor rule: 例句 weight 0, 力具 weight 1
    let mut t = HashMap::new();
    t.insert("ㄌㄧˋ-ㄐㄩˋ|例句".to_string(), 0.0);
    t.insert("ㄌㄧˋ-ㄐㄩˋ|力具".to_string(), 1.0);
    a.set_short_table(Some(t));
    assert_eq!(text(&a.rebeam(&base, &[], 0.0)), "力具");
    assert_eq!(text(&a.rebeam_short(&base, &[], 0.0, false, cfg(4, 50.0), false)), "例句", "table off: T1");
    // a lock that contradicts the word drops the jump
    let locked = a.rebeam(&base, &[Some('立'), None], 0.0);
    assert_eq!(locked.chars[0], Some('立'));
}

#[test]
fn candidate_scores_bit_identical_when_the_schedule_is_flat() {
    // lambda_0 = lambda_L, beta 0, F3 off: the candidate scores keep the old expression bit for bit (no prior here: the
    // window term is skipped, so this pins the D(w) model part; the prior part is the same branch by prior_plain)
    let mut a = assets();
    a.prior = Some(crate::prior::tests::tiny()); // counts over ids 2..4: some candidate windows get a non-zero prior term
    let s = syl("ㄓㄠˇ ㄌㄧˋ ㄐㄩˋ");
    let c = a.convert("找", &s, &[], 0.5);
    let p = crate::CandParams { lambda: 0.5, ..crate::CandParams::default() };
    let key = |a: &Assets, c: &Conversion| a.candidates(c, 3, &p).into_iter().map(|x| (x.text, x.span, x.score.to_bits())).collect::<Vec<_>>();
    let old = key(&a, &c);
    a.set_short(ShortCfg { c: 4, l0: Some(0.5), wb: 0.0, ctx: false }).unwrap();
    assert!(a.short_view(&c.inf).prior_plain(0.5) && !a.short_view(&c.inf).prior_plain(0.7));
    assert_eq!(key(&a, &c), old, "flat schedule: same items, order and score bits");
    // the same conversion, F3 off, only lambda_0 changed: the prior term of D(w) is weighted by the schedule
    a.set_short(ShortCfg { c: 4, l0: Some(3.0), wb: 0.0, ctx: false }).unwrap();
    let moved = key(&a, &c);
    assert!(old.iter().any(|(t, sp, b)| moved.iter().any(|(t2, sp2, b2)| t == t2 && sp == sp2 && b != b2)));
}

#[test]
fn rebeam_short_default_equals_product_path_off() {
    let a = assets();
    for (ctx, s) in [("", "ㄌㄧˋ ㄐㄩˋ"), ("找", "ㄓㄠˇ ㄌㄧˋ ㄐㄩˋ")] {
        let c = a.convert(ctx, &syl(s), &[], 0.0);
        assert_eq!(a.rebeam_short(&c, &[], 0.0, false, ShortCfg::default(), false).chars, a.rebeam(&c, &[], 0.0).chars);
    }
}
