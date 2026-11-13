use super::*;
use crate::{convert::Inference, pn::{BuildWord, PnLexicon, PnCfg}, NormCfg};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering as AtomicOrdering}};
static NEXT: AtomicUsize = AtomicUsize::new(0);
const DICT: &str = "ㄚ 甲 -1\nㄚ 乙 -2\nㄚ 丙 -3\nㄚ 丁 -4\nㄚ 台 -5\nㄚ 臺 -6\nㄚ 宣 -7\nㄚ 佈 -8\nㄚ 布 -9\nㄊㄚ 她 -1\nㄊㄚ 他 -2\nㄕˋ 事 -1\nㄕˋ 試 -2\nㄧㄝˋ 液 -1\nㄕˊ 石 -1\nㄖㄨㄟˇ 蕊 -1\n";
fn lex(a: &Assets, ws: &[(&str, &str, u8)]) -> Arc<PnLexicon> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/lexux/model-tests")
        .join(format!("{}-{}", std::process::id(), NEXT.fetch_add(1, AtomicOrdering::Relaxed)));
    std::fs::create_dir_all(&root).unwrap();
    let base = root.join("lex");
    let ws = ws.iter().map(|(w, r, c)| BuildWord { word: w.to_string(), reading: r.to_string(), class: *c }).collect::<Vec<_>>();
    pn::build(a, &ws, &base).unwrap();
    let l = Arc::new(PnLexicon::open(&base, a).unwrap());
    // Windows mapped files live until Assets drops; fixtures stay isolated under data/lexux.
    l
}
fn setup(ws: &[(&str, usize, u8)], n: usize) -> (Assets, Conversion) {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    let words: Vec<_> = ws.iter().map(|(w, _, c)| (*w, vec!["ㄚ"; w.chars().count()].join("-"), *c)).collect();
    a.set_gen(Some(lex(&a, &words.iter().map(|(w, r, c)| (*w, r.as_str(), *c)).collect::<Vec<_>>())));
    let syllables = vec!["ㄚ".to_string(); n];
    let sets: Vec<_> = syllables.iter().map(|s| a.legal[s].clone()).collect();
    let scores = sets.iter().map(|s| s.iter().map(|&id| if a.vocab.char_of(id) == Some('甲') { 1.0 } else { 0.0 }).collect()).collect();
    let inf = Arc::new(Inference { recent_output: false, syllables, sets, scores, base_fallback: vec![], fallback: false, cache: None, ctx_len: 0, ctx_tail: vec![] });
    let c = Conversion { chars: vec![], fallback: false, inf, forced: vec![], hints: vec![], auto: vec![], selection: None };
    let c = a.rebeam_ex(&c, &[], 0.0, false);
    (a, c)
}
fn cfg(a: f64, b: Option<f64>, w: bool) -> Option<Config> { Some(Config { a, b, w }) }
fn raw_conversion(a: &Assets, syllables: Vec<String>, raw: &str, style: bool) -> Conversion {
    let sets: Vec<_> = syllables.iter().map(|s|a.legal[s].clone()).collect();
    let raw: Vec<_> = raw.chars().collect();
    let scores = sets.iter().enumerate().map(|(k,s)|s.iter().map(|&id|if a.vocab.char_of(id)==Some(raw[k]) {1.} else {0.}).collect()).collect();
    let inf = Arc::new(Inference{recent_output:false,syllables,sets,scores,base_fallback:vec![],fallback:false,cache:None,ctx_len:0,ctx_tail:vec![]});
    let c = Conversion{chars:vec![],fallback:false,inf,forced:vec![],hints:vec![],auto:vec![],selection:None};
    a.rebeam_ex(&c,&[],0.,style)
}
fn apply(a: &Assets, c: &Conversion, ac: f64, b: Option<f64>, w: bool) -> Conversion { a.apply_lexux(c, cfg(ac,b,w), false) }
#[test] fn grid_and_missing_keys() {
    assert_eq!(Config::grid().len(), 104);
    let mut m = HashMap::new(); assert_eq!(Config::parse(&m), Ok(None));
    m.insert("lexux_a".into(), "2".into()); assert_eq!(Config::parse(&m), Ok(None));
    m.insert("lexux_b".into(), "NaN".into()); assert!(Config::parse(&m).is_err());
    m.insert("lexux_b".into(), "off".into()); assert_eq!(Config::parse(&m), Ok(cfg(2.,None,false)));
    m.insert("lexux_w".into(), "on".into()); assert_eq!(Config::parse(&m), Ok(cfg(2.,None,true)));
}
#[test] fn boundaries() {
    let (a,c) = setup(&[("乙乙乙",0,JUMP)],3);
    let lp = pn::log_softmax(&c.inf.scores);
    let model = |i:usize,id| lp[i][c.inf.sets[i].binary_search(&id).unwrap()];
    let y = &c.selection.as_ref().unwrap().ids;
    let d = score(y,0,y,&model,&|_|0.,&[],&|_,_|0.) - score(y,0,&vec![a.vocab_id('乙');3],&model,&|_|0.,&[],&|_,_|0.);
    assert!((d-3.).abs()<1e-12);
    assert!(apply(&a,&c,d-1e-9,Some(0.),false).hints.is_empty());
    let at_a = apply(&a,&c,d,Some(0.),false); assert_eq!(at_a.hints.len(),1); assert!(at_a.auto.is_empty());
    assert!(apply(&a,&c,4.,Some(d),false).auto.is_empty());
    assert_eq!(apply(&a,&c,4.,Some(d+1e-9),false).auto.len(),1);
}
#[test] fn two_char_and_jump_and_w_and_off() {
    let (a,c) = setup(&[("乙乙",0,JUMP)],2);
    assert!(apply(&a,&c,100.,Some(100.),true).auto.is_empty());
    assert_eq!(apply(&a,&c,100.,Some(100.),true).hints.len(),1);
    let (a,c) = setup(&[("乙乙乙",0,0)],3);
    assert!(apply(&a,&c,100.,Some(100.),true).auto.is_empty());
    let (a,c) = setup(&[("乙乙乙",0,JUMP)],3);
    assert!(apply(&a,&c,4.,Some(0.),false).auto.is_empty());
    assert_eq!(apply(&a,&c,4.,Some(0.),true).auto.len(),1);
    assert!(apply(&a,&c,4.,None,true).auto.is_empty());
    assert!(apply(&a,&c,2.,Some(0.),true).hints.is_empty());
}
#[test] fn w_checks_dictionary_gen_pn_and_user_words() {
    let (mut a,c) = setup(&[("乙乙乙",0,JUMP),("甲甲甲",0,JUMP)],3);
    assert!(apply(&a,&c,4.,Some(0.),true).auto.is_empty());
    a.set_gen(Some(lex(&a,&[("乙乙乙","ㄚ-ㄚ-ㄚ",JUMP)])));
    let (mut d,_) = crate::lexicon::Dictionary::parse("ㄚ-ㄚ-ㄚ 甲甲甲 -1\n");
    d.apply_aliases("").unwrap(); a.dict = d;
    assert!(apply(&a,&c,4.,Some(0.),true).auto.is_empty());
}
#[test] fn overlap_chain_and_adjacent() {
    let (a,c) = setup(&[("乙乙乙",0,JUMP)],7);
    let x = apply(&a,&c,100.,Some(100.),false);
    assert_eq!(x.auto, vec![(0,3),(3,3)]);
    assert_eq!(x.hints.len(),2);
    assert_eq!(x.chars[6],c.chars[6]);
    // C is never resurrected after deletion by B; A and C can both survive when B loses.
    for h in &x.hints { assert_eq!(h.options.len(),2); assert!(h.auto); assert!(h.options.iter().all(|o| o.reading.is_none())); }
}
#[test] fn options_order_cap_and_final_zero() {
    let (a,c) = setup(&[("乙乙",0,0),("丙丙",0,0),("丁丁",0,0)],2);
    let x = apply(&a,&c,100.,None,false);
    assert_eq!(x.hints.len(),1); assert_eq!(x.hints[0].options.len(),3); assert!(x.hints[0].options.iter().all(|o| o.reading.is_none()));
    assert_eq!(x.hints[0].options[0].display,c.text().unwrap());
    let mut expected = ["乙乙","丙丙","丁丁"].map(|s| (s.chars().map(|c| a.vocab_id(c)).collect::<Vec<_>>(),s));
    expected.sort(); assert_eq!(x.hints[0].options[1].display,expected[0].1);
}
#[test] fn blocked_real_forced_and_user_jump_masks() {
    let (a,c) = setup(&[("乙乙乙",0,JUMP)],3);
    for k in 0..3 {
        let mut x = c.clone(); Arc::make_mut(x.selection.as_mut().unwrap()).blocked[k] = true;
        let x = apply(&a,&x,100.,Some(100.),true); assert!(x.auto.is_empty() && x.hints.is_empty());
    }
    let locked = a.rebeam_ex(&c,&[Some('甲')],0.,false);
    assert!(apply(&a,&locked,100.,Some(100.),true).hints.is_empty());
}
#[test] fn pn_partial_lock_tai_fold_protects_unlocked_part() {
    let (mut a,mut c) = setup(&[("乙乙",0,JUMP)],3);
    a.set_norm(NormCfg::ALL);
    a.set_pn(Some(lex(&a,&[("臺甲","ㄚ-ㄚ",0)])),PnCfg::default());
    let s = Arc::make_mut(c.selection.as_mut().unwrap()); s.ids[0] = a.vocab_id('台'); s.blocked[0] = true;
    assert!(apply(&a,&c,100.,Some(100.),true).hints.is_empty());
}
#[test] fn tai_equivalence_does_not_pair_model_scores() {
    let (mut a,mut c) = setup(&[("臺臺",0,JUMP)],2);
    a.set_norm(NormCfg::ALL);
    Arc::make_mut(c.selection.as_mut().unwrap()).ids.fill(a.vocab_id('台'));
    assert!(apply(&a,&c,100.,Some(100.),false).hints.is_empty());
    // Direct raw score: no fold or cands score pairing is permitted.
    let lp = |_: usize,id| if id == 1 { -2. } else { -5. };
    assert_eq!(score(&[1,1],0,&[1,1],&lp,&|_|0.,&[],&|_,_|99.) - score(&[1,1],0,&[2,2],&lp,&|_|0.,&[],&|_,_|99.),6.);
}
#[test] fn display_dedup_and_auto_original() {
    let (a,mut c) = setup(&[("乙乙乙",0,JUMP)],3);
    Arc::make_mut(c.selection.as_mut().unwrap()).display.fill(Some('乙'));
    let x = apply(&a,&c,100.,None,false); assert!(x.hints.is_empty());
    let x = apply(&a,&c,100.,Some(100.),false); assert_eq!(x.auto.len(),1); assert!(x.hints.is_empty());
    let (a,mut c) = setup(&[("乙乙乙",0,JUMP)],5);
    Arc::make_mut(c.selection.as_mut().unwrap()).display[4] = Some('他');
    let x = apply(&a,&c,100.,Some(100.),false); assert_eq!(x.chars[4],Some('他'));
    assert_eq!(x.hints[0].options[1].display,"甲甲甲");
}
#[test] fn baseline_norm_ta_boundary_and_option_zero() {
    let (a,mut c) = setup(&[("乙乙",0,JUMP)],2);
    Arc::make_mut(c.selection.as_mut().unwrap()).display = vec![Some('他'),Some('臺')];
    assert_eq!(apply(&a,&c,100.,None,false).hints[0].options[0].display,"他臺");
}
#[test] fn real_norm_word_boundary_and_ta_preserve_baseline() {
    let (mut a,_) = setup(&[("乙乙乙",0,JUMP)],5);
    a.set_norm(NormCfg::ALL); a.set_ta_default(true);
    let c = raw_conversion(&a,["ㄚ","ㄚ","ㄚ","ㄚ","ㄊㄚ"].map(String::from).to_vec(),"甲甲宣佈她",false);
    assert_eq!(c.text().unwrap(),"甲甲宣布他");
    let x = apply(&a,&c,100.,Some(100.),false);
    assert_eq!(x.auto,vec![(0,3)]); assert_eq!(x.text().unwrap(),"乙乙乙布他");
    assert_eq!(x.hints[0].options[1].display,"甲甲宣");
}
#[test] fn actual_option_zero_ta_and_tai_style() {
    let mut a = Assets::from_dictionary_text(DICT,"").unwrap();
    a.set_norm(NormCfg::ALL); a.set_ta_default(true);
    a.set_gen(Some(lex(&a,&[("乙她","ㄚ-ㄊㄚ",JUMP)])));
    let c = raw_conversion(&a,vec!["ㄚ".into(),"ㄊㄚ".into()],"甲她",false);
    let x = apply(&a,&c,100.,None,false); assert_eq!(x.hints[0].options[0].display,"甲他");
    assert_eq!(x.hints[0].options[1].display,"乙她");
    a.set_gen(Some(lex(&a,&[("乙臺","ㄚ-ㄚ",JUMP)])));
    let c = raw_conversion(&a,vec!["ㄚ".into(),"ㄚ".into()],"甲台",true);
    assert_eq!(apply(&a,&c,100.,None,false).hints[0].options[0].display,"甲臺");
}
#[test] fn fallback_incomplete_recent_and_missing_are_identity() {
    let (mut a,c) = setup(&[("乙乙乙",0,JUMP)],3);
    let off = a.apply_lexux(&c,None,false); assert_eq!(off.chars,c.chars); assert!(off.hints.is_empty());
    let mut x = c.clone(); x.fallback = true; assert!(apply(&a,&x,100.,Some(100.),true).hints.is_empty());
    assert!(a.apply_lexux(&c,cfg(100.,Some(100.),true),true).hints.is_empty());
    Arc::get_mut(&mut x.inf).map(|_|()); // shared inference intentionally immutable.
    a.set_lexux(cfg(100.,Some(100.),true)).unwrap();
    let recent = a.convert_cached_source("",None,&vec!["ㄚ".into(); 3],&[],0.,false,true);
    assert!(recent.hints.is_empty());
    let fallback = a.fallback(&["missing".into()],&[]); assert!(!fallback.complete()); assert!(fallback.hints.is_empty());
    assert_eq!(a.rebeam_ex(&c,&[],0.,false).auto.len(),1);
}
#[test] fn configured_engine_and_saved_selection_share_rules() {
    for n in [2,3,6] {
        let (mut a,c) = setup(&[("乙乙",0,JUMP),("丙丙丙",0,JUMP)],n);
        for config in [None,cfg(2.,None,false),cfg(100.,Some(100.),false),cfg(4.,Some(0.),true)] {
            a.set_lexux(config).unwrap();
            let direct = a.rebeam_ex(&c,&[],0.,false);
            let saved = a.apply_lexux(&c,config,false);
            assert_eq!(direct.chars,saved.chars); assert_eq!(direct.auto,saved.auto); assert_eq!(direct.hints,saved.hints);
        }
    }
}
#[test] fn prior_history_manual_and_truncated_suffix() {
    let y = [1,1,1,1,1];
    let prior = |h: &[u32],id| -((h.iter().sum::<u32>() + id) as f64);
    let lp = |_: usize,id| -(id as f64);
    // Replacement at 0 affects i=0..3; last three-history term differs by one each.
    let d = |pre: &[u32]| score(&y,0,&[1],&lp,&|_|0.5,pre,&prior) - score(&y,0,&[2],&lp,&|_|0.5,pre,&prior);
    assert_eq!(d(&[]),3.); assert_eq!(d(&[7,8,9]),3.);
    let seen = std::cell::RefCell::new(vec![]);
    score(&y,0,&[2],&lp,&|_|0.5,&[7,8,9],&|h,id| { seen.borrow_mut().push((h.to_vec(),id)); prior(h,id) });
    assert_eq!(*seen.borrow(),vec![(vec![7,8,9],2),(vec![8,9,2],1),(vec![9,2,1],1),(vec![2,1,1],1)]);
    assert_eq!(score(&y,4,&[2],&lp,&|_|0.5,&[],&prior),-4.5);
    assert_eq!(score(&y,4,&[2],&lp,&|_|0.,&[],&prior),-2.);
}
#[test] fn prior_ctx_switch_manual_difference() {
    let (a,mut c) = setup(&[("乙乙乙",0,JUMP)],5);
    Arc::get_mut(&mut c.inf).unwrap().ctx_tail = vec![7,8,9];
    let p = |h:&[u32],id| -(h.iter().sum::<u32>() as f64)*(id as f64);
    let lp = |_:usize,id| -(id as f64);
    for ctx in [false,true] {
        let sp = a.short_view_of(prior::ShortCfg {ctx,..Default::default()},false,&c.inf);
        assert_eq!(sp.pre(),if ctx {&[7,8,9][..]} else {&[][..]});
        let y = [1,1,1,1,1];
        let d = score(&y,0,&[1],&lp,&|i|sp.weight(0.5,i),sp.pre(),&p)-score(&y,0,&[2],&lp,&|i|sp.weight(0.5,i),sp.pre(),&p);
        assert_eq!(d,if ctx {14.5} else {2.5});
    }
}
#[test] fn adjacent_recomputed_and_deleted_never_revived() {
    let hs = vec![Hit{s:0,ids:vec![2;3],class:JUMP},Hit{s:3,ids:vec![3;3],class:JUMP}];
    let cfg = Config{a:10.,b:Some(0.),w:false};
    let delta = |y:&[u32],h:&Hit| if h.s==0 {-2.} else if y[0]==2 {-1.} else {1.};
    let (_,_,accepted) = automatic(&hs,&[1;6],cfg,&delta,&|_,_|false);
    assert_eq!(accepted.iter().map(|h|h.s).collect::<Vec<_>>(),vec![0,3]);
    let delta = |y:&[u32],h:&Hit| if h.s==0 {-2.} else if y[0]==2 {1.} else {-1.};
    assert_eq!(automatic(&hs,&[1;6],cfg,&delta,&|_,_|false).2.len(),1);
    let chain = vec![Hit{s:0,ids:vec![2;3],class:JUMP},Hit{s:2,ids:vec![3;3],class:JUMP},Hit{s:4,ids:vec![4;3],class:JUMP}];
    let (_,deleted,accepted) = automatic(&chain,&[1;7],cfg,&|_,h|-(4-h.s as i32).abs() as f64-1.,&|_,_|false);
    assert_eq!(accepted.iter().map(|h|h.s).collect::<Vec<_>>(),vec![0,4]); assert!(deleted.iter().all(|&b|b));
    let (_,_,accepted) = automatic(&chain,&[1;7],cfg,&|_,h|if h.s==2 {-5.} else {-1.},&|_,_|false);
    assert_eq!(accepted.iter().map(|h|h.s).collect::<Vec<_>>(),vec![2]);
}
#[test] fn forced_and_two_character_user_selection_integration() {
    let (mut a,c) = setup(&[("丙丙丙",0,JUMP)],3);
    a.set_user_rules(true,10.).unwrap();
    let w = a.user_word("乙乙乙",&c.inf.syllables,10.).unwrap();
    a.set_user(Some(Arc::new(a.build_user_lexicon([w]))));
    a.set_lexux(cfg(100.,Some(100.),true)).unwrap();
    let x = a.rebeam_ex(&c,&[],0.,false);
    assert_eq!(x.forced.len(),1); assert!(x.hints.is_empty());
    let (mut a,c) = setup(&[("丙丙丙",0,JUMP)],3);
    a.set_user_rules(true,10.).unwrap();
    let w = a.user_word("乙乙",&c.inf.syllables[..2],10.).unwrap();
    a.set_user(Some(Arc::new(a.build_user_lexicon([w]))));
    a.set_lexux(cfg(100.,Some(100.),true)).unwrap();
    let x = a.rebeam_ex(&c,&[],0.,false);
    assert!(x.forced.is_empty()); assert_eq!(x.chars[..2],vec![Some('乙');2]); assert!(x.hints.is_empty());
}
#[test] fn equal_sort_keys_length_start_ids_and_overlapping_hints() {
    let hit = |s,n,id|Hit{s,ids:vec![id;n],class:JUMP};
    let mut hs = vec![(hit(0,2,1),0.),(hit(1,3,1),0.),(hit(0,3,2),0.),(hit(0,3,1),0.)];
    hs.sort_by(order);
    assert_eq!((hs[0].0.s,hs[0].0.ids.len(),hs[0].0.ids[0]),(0,3,1));
    assert_eq!(hs[1].0.ids[0],2); assert_eq!(hs[2].0.s,1); assert_eq!(hs[3].0.ids.len(),2);
    let (a,c) = setup(&[("乙乙",0,0)],4);
    let x = apply(&a,&c,100.,None,false);
    assert_eq!(x.hints.iter().map(|h|h.start).collect::<Vec<_>>(),vec![0,2]);
}
#[test] fn litmus_two_character_hint() {
    let mut a = Assets::from_dictionary_text(DICT,"").unwrap();
    a.set_gen(Some(lex(&a,&[("試液","ㄕˋ-ㄧㄝˋ",JUMP)])));
    let syllables: Vec<String> = ["ㄕˊ","ㄖㄨㄟˇ","ㄕˋ","ㄧㄝˋ"].map(String::from).to_vec();
    let sets: Vec<_> = syllables.iter().map(|s| a.legal[s].clone()).collect();
    let scores = sets.iter().map(|s| s.iter().map(|&i| if a.vocab.char_of(i) == Some('事') {1.} else {0.}).collect()).collect();
    let inf = Arc::new(Inference { recent_output: false, syllables,sets,scores,base_fallback:vec![],fallback:false,cache:None,ctx_len:0,ctx_tail:vec![] });
    let c = Conversion { chars:vec![],fallback:false,inf,forced:vec![],hints:vec![],auto:vec![],selection:None };
    let c = a.rebeam_ex(&c,&[],0.,false); let x = apply(&a,&c,10.,Some(10.),true);
    assert_eq!(x.chars,c.chars); assert_eq!(x.hints.len(),1); assert_eq!(x.hints[0].start,2); assert!(x.auto.is_empty());
}

#[path = "lexux_frozen.rs"]
mod frozen;
#[test]
fn missing_keys_match_pre_lexux_frozen_bytes() {
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/fixtures/r1-baseline-expected.json")).unwrap();
    #[allow(unused_mut)]
    let (mut expected, mut actual) = (expected, serde_json::Value::Array(frozen::snapshots(|a| {
        a.set_lexux(Config::parse(&HashMap::new()).unwrap()).unwrap();
    })));
    // The score bits come from the Windows C runtime (exp/ln); elsewhere the characters, flags and sets are compared.
    #[cfg(not(windows))]
    for side in [&mut expected, &mut actual] {
        for s in side.as_array_mut().unwrap() {
            s.as_object_mut().unwrap().remove("score_bits");
        }
    }
    // Suppress fixture bytes on failure, even though these are synthetic cases.
    assert!(serde_json::to_vec(&actual).unwrap() == serde_json::to_vec(&expected).unwrap(), "frozen snapshot mismatch");
}
#[test]
fn model_mode_empty_legal_position_is_unchanged() {
    let (mut a,mut c) = setup(&[("乙乙乙",0,JUMP)],3);
    a.model = Some(crate::n1::N1::random(a.vocab.len(),32,1,20262871));
    assert!(!apply(&a,&c,100.,Some(100.),true).hints.is_empty());
    let inf = Arc::get_mut(&mut c.inf).unwrap();
    inf.sets[1].clear(); inf.scores[1].clear();
    a.set_lexux(cfg(100.,Some(100.),true)).unwrap();
    let x = a.rebeam_ex(&c,&[],0.,false);
    assert!(!x.fallback && !x.inf.fallback);
    assert!(x.inf.sets[1].is_empty() && x.chars[1].is_none());
    assert!(x.selection.is_none() && x.hints.is_empty() && x.auto.is_empty());
    let y = a.apply_lexux(&x,cfg(100.,Some(100.),true),false);
    assert!(y.chars == x.chars && y.hints.is_empty() && y.auto.is_empty());
}
