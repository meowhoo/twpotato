use super::*;
use crate::{n1::N1, NormCfg};

fn cfg() -> GenjCfg { GenjCfg {b3:[Some(1.0),Some(2.0),None,Some(6.0)],h:Some(1)} }
fn fixture() -> (Vec<Vec<u32>>,Vec<Vec<f64>>,Vec<Option<u32>>) {
    (vec![vec![2,3],vec![2,3],vec![2,3]],vec![vec![-1.0,-2.0];3],vec![None;3])
}
#[test]
fn tier_off_and_one_h_across_tiers() {
    let (sets,lp,locked)=fixture();
    let raw=vec![(0,0,vec![3,3,3]),(0,1,vec![2,2,2]),(0,2,vec![2,2,3]),(0,3,vec![2,3,2])];
    let j=jumps(&raw,&sets,&lp,&locked,&[],&cfg(),None);
    assert_eq!(j,vec![Jump{start:0,ids:vec![2,2,2],bonus:2.0}]);
    let mut c=cfg();c.h=None;
    assert_eq!(jumps(&raw,&sets,&lp,&locked,&[],&c,None).len(),3);
}
#[test]
fn duplicate_max_bonus_and_canonical_order() {
    let a=Jump{start:0,ids:vec![2,2,2],bonus:8.0};
    let b=Jump{bonus:9.0,..a.clone()};
    assert_eq!(merge(vec![a],vec![b.clone(),b.clone()]),vec![b]);
}
#[test]
fn folded_lock_uses_locked_id_and_forced_spans_block() {
    let (sets,lp,mut locked)=fixture();locked[1]=Some(3);
    let raw=vec![(0,0,vec![2,2,2])];
    assert!(jumps(&raw,&sets,&lp,&locked,&[],&cfg(),None).is_empty());
    let j=jumps(&raw,&sets,&lp,&locked,&[],&cfg(),Some((3,2)));
    assert_eq!(j[0].ids,vec![2,3,2]);
    for mask in [vec![true,false,false],vec![false,true,false],vec![false,false,true]] {
        assert!(jumps(&raw,&sets,&lp,&locked,&mask,&cfg(),Some((3,2))).is_empty());
    }
}
#[test]
fn length_and_legal_set_restrictions() {
    let (sets,lp,locked)=fixture();
    assert!(jumps(&[(0,0,vec![2,2]),(0,0,vec![2,2,4])],&sets,&lp,&locked,&[],&cfg(),None).is_empty());
}
#[test]
fn strict_settings() {
    let mut m=HashMap::from([("b3_a".into(),"1".into()),("b3_b".into(),"2".into()),("b3_c".into(),"off".into()),("b3_d".into(),"6".into()),("h".into(),"4".into())]);
    assert!(GenjCfg::parse(&m,"h").is_ok());
    for bad in ["NaN","inf","-1","bad"] {m.insert("b3_a".into(),bad.into());assert!(GenjCfg::parse(&m,"h").is_err());}
    m.insert("b3_a".into(),"0".into());m.insert("h".into(),"0".into());assert!(GenjCfg::parse(&m,"h").is_err());
    m.remove("h");assert!(GenjCfg::parse(&m,"h").is_err());
}

fn tiny() -> Assets {
    let mut a=Assets::from_dictionary_text("ㄅㄚ 八 -1\nㄅㄚ 巴 -2\nㄊㄞˊ 台 -1\nㄊㄞˊ 臺 -2\n","").unwrap();
    a.model=Some(N1::random(a.vocab.len(),32,1,17));a.set_norm(NormCfg::ALL);a
}
#[test]
fn assets_validation_and_j_alone_product_rebeam() {
    let mut a=tiny();
    let root=std::env::temp_dir().join(format!("srchj-test-{}",std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let base=root.join("genj");
    assert!(open(&base,&a).unwrap().is_none());
    super::super::build(&a,&[super::super::BuildWord{word:"巴巴巴".into(),reading:"ㄅㄚ-ㄅㄚ-ㄅㄚ".into(),class:3}],&base).unwrap();
    assert!(open(&base,&a).is_err());
    std::fs::write(base.with_extension("cfg"),"b3_a=1\nb3_b=2\nb3_c=off\nb3_d=100\nh=4\n").unwrap();
    for invalid in [vec![],vec![3,3],vec![4],vec![2]] {
        std::fs::write(base.with_extension("tier"),invalid).unwrap();assert!(open(&base,&a).is_err());
    }
    std::fs::write(base.with_extension("tier"),[3]).unwrap();
    a.set_genj(open(&base,&a).unwrap());assert!(a.pn().is_none());
    let syl=vec!["ㄅㄚ".to_string();3];
    let on=a.convert("",&syl,&[],0.0);
    assert!(on.text()==Some("巴巴巴".into()));
    assert_eq!(on.chars,a.rebeam(&on,&[],0.0).chars);
    let locked=a.rebeam(&on,&[None,Some('八'),None],0.0);
    assert_eq!(locked.chars[1],Some('八'));
    a.set_genj(None);
    let off=a.rebeam(&on,&[],0.0);
    let plain=tiny().convert("",&syl,&[],0.0);
    assert_eq!(off.chars,plain.chars);
    drop(a);
    assert!(root.starts_with(std::env::temp_dir()));std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tied_log_scores_choose_ids_before_bonus() {
    let (sets,mut lp,locked)=fixture();lp.fill(vec![-1.0,-1.0]);
    let raw=vec![(0,0,vec![2,2,2]),(0,3,vec![3,3,3])];
    let j=jumps(&raw,&sets,&lp,&locked,&[],&cfg(),None);
    assert_eq!(j[0].ids,vec![2,2,2]);assert_eq!(j[0].bonus,1.0);
}

#[test]
fn j_builder_retains_known_readings_without_changing_pn_builder() {
    let a=tiny();
    let root=std::env::temp_dir().join(format!("srchj-build-test-{}",std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    // All ids are known, but the first character is illegal for the first syllable.
    let words=[super::super::BuildWord {word:"台巴巴".into(),reading:"ㄅㄚ-ㄅㄚ-ㄅㄚ".into(),class:0}];
    let st=super::super::build(&a,&words,&root.join("pn")).unwrap();assert_eq!(st.illegal,1);
    let st=super::super::build_j(&a,&words,&root.join("j")).unwrap();assert_eq!(st.kept,1);
    let unknown=[super::super::BuildWord {word:"巴巴巴".into(),reading:"ㄅㄧㄤˊ-ㄅㄚ-ㄅㄚ".into(),class:0}];
    let st=super::super::build_j(&a,&unknown,&root.join("unknown")).unwrap();assert_eq!(st.kept,1);
    let lex=PnLexicon::open(&root.join("unknown"),&a).unwrap();
    assert!(lex.hits(&[None,Some(a.vocab.syllable_id("ㄅㄚ") as u16),Some(a.vocab.syllable_id("ㄅㄚ") as u16)]).is_empty());
    assert!(root.starts_with(std::env::temp_dir()));std::fs::remove_dir_all(root).unwrap();
}
