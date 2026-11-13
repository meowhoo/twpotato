// Shared numeric snapshots: pre-LEXUX convert.rs/lib.rs produce the frozen oracle.
use crate::{Assets, Conversion, NormCfg, n1::N1, pn};
use serde_json::{Value, json};
const DICT: &str = "ㄚ 甲 -1\nㄚ 乙 -2\nㄚ 丙 -3\nㄚ 丁 -4\nㄚ 台 -5\nㄚ 臺 -6\nㄚ 宣 -7\nㄚ 佈 -8\nㄚ 布 -9\nㄊㄚ 她 -1\nㄊㄚ 他 -2\nㄕˋ 事 -1\nㄕˋ 試 -2\nㄧㄝˋ 液 -1\nㄕˊ 石 -1\nㄖㄨㄟˇ 蕊 -1\n";
pub fn snapshots(configure: impl Fn(&mut Assets)) -> Vec<Value> {
    let mut a = Assets::from_dictionary_text(DICT, "").unwrap();
    a.set_norm(NormCfg::ALL);
    a.set_ta_default(true);
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/lexux")
        .join(format!("frozen-probe-{}",std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let base = directory.join("lex");
    pn::build(&a,&[pn::BuildWord{word:"乙乙乙".into(),reading:vec!["ㄚ";3].join("-"),class:0xc0}],&base).unwrap();
    a.set_gen(Some(std::sync::Arc::new(pn::PnLexicon::open(&base,&a).unwrap())));
    configure(&mut a);
    let mut out = vec![];
    for model in [false,true] {
        a.model = model.then(|| N1::random(a.vocab.len(),32,1,20262871));
        for n in [2,3,6] {
            for style in [false,true] {
                for locked in [false,true] {
                    for incomplete in [false,true] {
                        let mut syllables = vec!["ㄚ".to_string();n];
                        if incomplete { syllables[1] = "missing".into(); }
                        let ch = a.vocab.char_of(a.legal["ㄚ"][0]).unwrap();
                        let locks = if locked { vec![Some(ch)] } else { vec![] };
                        let c = a.convert_ex("",&syllables,&locks,0.5,style);
                        assert_eq!(c.fallback,!model);
                        assert_eq!(c.complete(),!incomplete);
                        out.push(snapshot(&c));
                        out.push(snapshot(&a.rebeam_ex(&c,&locks,0.,style)));
                    }
                }
            }
        }
    }
    out
}
fn snapshot(c: &Conversion) -> Value {
    json!({"chars":c.chars.iter().map(|x|x.map(u32::from)).collect::<Vec<_>>(),
           "text_bytes":c.text().map(|s|s.into_bytes()),"fallback":c.fallback,
           "complete":c.complete(),"forced":c.forced,"sets":c.inf.sets,
           "score_bits":c.inf.scores.iter().map(|s|s.iter().map(|x|x.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>()})
}
