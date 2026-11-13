// Included by pneval: raw data is confined to the private interchange file.
// The Python gateway validates frozen inputs before this function loads any model or unit.
fn lexux_python(args: &[String]) -> i32 {
    std::process::Command::new("train/.venv/Scripts/python.exe")
        .args(["-B", "-X", "utf8", "ime/scripts/lexux_eval.py"])
        .args(args).status().expect("lexux scorer").code().unwrap_or(1)
}
fn lexux_run(args: &[String]) {
    use serde_json::{Value, json};
    assert_eq!(args.len(),3,"lexux request");
    if lexux_python(&["--start-decode".into(),args[2].clone()]) != 0 { std::process::exit(2) }
    let req: Value = serde_json::from_slice(&fs::read(&args[2]).expect("request")).expect("request json");
    let mut m = settings(req["settings"].as_str().unwrap());
    for k in ["lexux_a","lexux_b","lexux_w"] { m.remove(k); }
    let assets = load(&m,true);
    let lambda = m.get("lambda").map_or(0.5,|s|s.parse().unwrap());
    let cfgs: Vec<twime_model::lexux::Config> = req["configs"].as_array().unwrap().iter().map(|c| twime_model::lexux::Config {
        a:c["a"].as_f64().unwrap(), b:c["b"].as_f64(), w:c["w"].as_bool().unwrap()
    }).collect();
    assert!(cfgs.iter().all(|c|c.valid()));
    let wanted: HashSet<String> = req["ids"].as_array().unwrap().iter().map(|x|x.as_str().unwrap().to_string()).collect();
    let prefixes: HashSet<String> = req["prefix_ids"].as_array().unwrap().iter().map(|x|x.as_str().unwrap().to_string()).collect();
    let mut output = std::io::BufWriter::new(fs::OpenOptions::new().write(true).create_new(true).open(req["raw"].as_str().unwrap()).unwrap());
    let mut count = 0usize;
    for path in req["sources"].as_array().unwrap() {
        for u in units(path.as_str().unwrap()).into_iter().filter(|u|wanted.contains(&u.id)) {
            let (base,_) = convert(&assets,&u,lambda);
            // Full unit is decoded once; its full prefix reuses that same Conversion.
            let mut write_row = |n: usize, c: &Conversion, full: bool| {
                let xs: Vec<Value> = cfgs.iter().map(|cfg| {
                    let x = assets.apply_lexux(c,Some(*cfg),false);
                    let hs: Vec<Value> = x.hints.iter().map(|h|json!({"s":h.start,"n":h.len,"options":h.options.iter().map(|o| &o.display).collect::<Vec<_>>(),"auto":h.auto})).collect();
                    // Layer attribution uses the first accepted option's exact reading and original spelling.
                    let layers = assets.lexux_hint_layers(&x);
                    json!({"text":x.text(),"auto":x.auto,"hints":hs,"layers":layers})
                }).collect();
                writeln!(output,"{}",json!({"id":u.id,"n":n,"full":full,"base":c.text(),"gold":u.gold.iter().collect::<String>(),"points":xs,"clean":assets.lexux_clean(&u.syl,&u.gold)})).unwrap();
            };
            if prefixes.contains(&u.id) {
                for n in 1..u.syl.len() {
                    let prefix = Unit { id:u.id.clone(),group:u.group.clone(),ctx:u.ctx.clone(),syl:u.syl[..n].to_vec(),gold:u.gold[..n].to_vec(),locks:u.locks[..n].to_vec() };
                    let (c,_) = convert(&assets,&prefix,lambda); write_row(n,&c,false);
                }
            }
            write_row(u.syl.len(),&base,true); count += 1;
        }
    }
    assert_eq!(count,wanted.len(),"lexux selected unit count");
    output.flush().unwrap();
    println!("decoded_units={count}");
}

/// New general lexicon only; preserves product files and checks the reserved bit before use.
fn lexux_build(args: &[String]) {
    assert_eq!(args.len(),5,"lexux-build settings source.tsv output-base");
    let base = std::path::Path::new(&args[4]);
    let root = std::fs::canonicalize("data/lexux").unwrap();
    let parent = std::fs::canonicalize(base.parent().unwrap()).unwrap();
    assert!(parent.starts_with(root),"lexux output directory");
    for ext in ["fst","lst"] {
        let path = PathBuf::from(format!("{}.{ext}",base.display()));
        assert!(fs::symlink_metadata(path).is_err(),"lexux build output exists");
    }
    let assets = load(&settings(&args[2]),false);
    let text = fs::read_to_string(&args[3]).unwrap();
    let mut lines = text.lines();
    let header: Vec<_> = lines.next().unwrap().split('\t').collect();
    let col = |k|header.iter().position(|&h|h==k).expect("lexux column");
    let (wi,ri,ti,ji) = (col("word"),col("reading"),col("tier"),col("jump"));
    let mut ws = vec![];
    for line in lines {
        let f: Vec<_> = line.split('\t').collect();
        let tier = match f[ti] { "甲"=>0,"乙"=>1,"丙"=>2,"丁"=>3,_=>panic!("lexux tier") };
        assert_eq!(tier & twime_model::lexux::JUMP,0);
        let jump = match f[ji] { "0"=>0,"1"=>twime_model::lexux::JUMP,_=>panic!("lexux jump") };
        ws.push(pn::BuildWord { word:f[wi].into(),reading:f[ri].into(),class:tier|jump|twime_model::lexux::META });
    }
    let s = pn::build(&assets,&ws,base).unwrap();
    println!("build_kept={} build_illegal={} build_duplicates={} build_bad_length={}",s.kept,s.illegal,s.duplicate,s.bad_length);
}
