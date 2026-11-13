// genjgrid/genjdump <settings> <units.tsv> <configs.txt> <out> <threads>
// Configs: a b c d h; stable names c0, c1, ...; all text remains in the output file.
fn genjgrid(a: &[String], dump: bool) {
    use pn::genj::GenjCfg;
    let m = settings(&a[2]);
    let lambda: f64 = m.get("lambda").map_or(0.5, |x|x.parse().unwrap());
    let cfgs: Vec<GenjCfg> = fs::read_to_string(&a[4]).unwrap().lines().filter_map(|l| {
        let l=l.split('#').next().unwrap().trim();
        if l.is_empty() { return None; }
        let f: Vec<_> = l.split_whitespace().collect();
        assert_eq!(f.len(),5,"genj config requires a b c d h");
        let cm: HashMap<_,_> = ["b3_a","b3_b","b3_c","b3_d","h"].into_iter().zip(f).map(|(k,v)|(k.into(),v.into())).collect();
        Some(GenjCfg::parse(&cm,"h").expect("genj config"))
    }).collect();
    let cfgs = Arc::new(cfgs);
    let us = Arc::new(units(&a[3]));
    let threads: usize = a[6].parse().unwrap();
    assert!((1..=4).contains(&threads));
    // Each worker owns the mutable configuration, while inference is retained per unit across configs.
    let mut hs=vec![];
    for t in 0..threads {
        let (m,cfgs,us)=(m.clone(),cfgs.clone(),us.clone());
        hs.push(std::thread::spawn(move || {
            let mut assets=load(&m,true);
            let j=assets.genj().map(|v|v.0.clone());
            assert!(j.is_some() || cfgs.is_empty(),"configs require J");
            let mut out=vec![];
            for (i,u) in us.iter().enumerate().skip(t).step_by(threads) {
                assets.set_genj(None);
                let (conv,style)=convert(&assets,u,lambda);
                let text=|c: &Conversion|c.chars.iter().map(|c|c.unwrap_or('\u{fffd}')).collect::<String>();
                let reference=text(&conv);
                out.push((0,i,format!("ref\t{}\t{reference}",u.id)));
                for (k,c) in cfgs.iter().enumerate() {
                    assets.set_genj(Some((j.as_ref().unwrap().clone(),c.clone())));
                    if dump {
                        let mut v=assets.genj_dump(&conv,&u.locks);
                        v["id"]=serde_json::json!(u.id);
                        v["cfg"]=serde_json::json!(k);
                        out.push((k+1,i,v.to_string()));
                    } else {
                        let c=assets.rebeam_ex(&conv,&u.locks,lambda,style);
                        let s=text(&c);
                        if s!=reference { out.push((k+1,i,format!("c{k}\t{}\t{s}",u.id))); }
                    }
                }
            }
            out
        }));
    }
    let mut rows=vec![];
    for h in hs { rows.extend(h.join().unwrap()); }
    rows.sort_by_key(|r|(r.0,r.1));
    let mut f=std::io::BufWriter::new(fs::File::create(&a[5]).unwrap());
    if dump {
        let assets=load(&m,true);
        writeln!(f,"{}",serde_json::json!({"meta":assets.genj_meta()})).unwrap();
    }
    if !dump { for (k,c) in cfgs.iter().enumerate() {
        let b: Vec<_>=c.b3.iter().map(|x|x.map_or_else(||"off".into(),|v|v.to_string())).collect();
        writeln!(f,"#cfg\tc{k}\t{}\t{}\t{}\t{}\t{}",b[0],b[1],b[2],b[3],c.h.map_or_else(||"inf".into(),|v|v.to_string())).unwrap();
    } }
    let mut n=0;
    for (k,_,s) in rows { if !dump || k>0 { writeln!(f,"{s}").unwrap(); n+=1; } }
    println!("genj units {} configs {} output_rows {n}",us.len(),cfgs.len());
}
