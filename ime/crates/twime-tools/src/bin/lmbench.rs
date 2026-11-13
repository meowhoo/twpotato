//! Offline-only PRED validation, scores and latency. Never logs corpus strings.
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    time::Instant,
};
use twime_model::lm::{Lm, Workspace};
#[derive(Deserialize)]
struct Row {
    key: String,
    obs: Vec<Obs>,
}
#[derive(Deserialize)]
struct Obs {
    h: String,
    lists: Vec<List>,
}
#[derive(Deserialize)]
struct List {
    k: usize,
    c: Vec<(String, Vec<String>, f64, bool)>,
}
#[derive(Deserialize)]
struct RefPart {
    context: Vec<u32>,
    scores: Vec<f64>,
    top9: Vec<usize>,
}
#[derive(Deserialize)]
struct Ref {
    ri: usize,
    oi: usize,
    candidates: Vec<Vec<u32>>,
    #[serde(rename = "S32")]
    s32: RefPart,
    #[serde(rename = "S61")]
    s61: RefPart,
}
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn rows(path: &str) -> Result<Vec<Row>> {
    BufReader::new(File::open(path)?)
        .lines()
        .map(|l| Ok(serde_json::from_str(&l?)?))
        .collect()
}
fn ids(s: &str, v: &HashMap<char, u32>) -> Vec<u32> {
    s.chars().map(|c| *v.get(&c).unwrap_or(&2)).collect()
}
fn tokenize(ob: &Obs, v: &HashMap<char, u32>, h: usize) -> (Vec<u32>, Vec<Vec<u32>>) {
    let mut context = ids(&ob.h, v);
    if context.len() > h {
        context.drain(..context.len() - h);
    }
    let adds = ob
        .lists
        .iter()
        .flat_map(|l| {
            l.c.iter().map(|(w, _, _, _)| {
                w.chars()
                    .skip(l.k)
                    .map(|c| *v.get(&c).unwrap_or(&2))
                    .collect()
            })
        })
        .collect();
    (context, adds)
}
fn order(ob: &Obs, s: &[f64]) -> Vec<usize> {
    let mut items = Vec::new();
    let mut i = 0;
    for l in &ob.lists {
        for (w, e, d, u) in &l.c {
            items.push((i, !u, *d, l.k + e.len(), w, e.join(" "), l.k));
            i += 1;
        }
    }
    items.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| s[b.0].total_cmp(&s[a.0]))
            .then_with(|| b.2.total_cmp(&a.2))
            .then(a.3.cmp(&b.3))
            .then(a.4.cmp(b.4))
            .then(a.5.cmp(&b.5))
            .then(b.6.cmp(&a.6))
    });
    items.into_iter().map(|x| x.0).collect()
}
fn report_file(path: &str, v: &Value) -> Result<()> {
    serde_json::to_writer_pretty(BufWriter::new(File::create(path)?), v)?;
    println!("{}", v);
    Ok(())
}
fn check(model: &Lm, raw: &[Row], precision: &str) -> Result<Value> {
    let mut ws = Workspace::default();
    let mut max = [0.0f64; 2];
    let mut same = [0usize; 2];
    let mut count = 0;
    let mut nc = 0;
    for l in BufReader::new(File::open("data/autocomp/aclm-ref.jsonl")?).lines() {
        let r: Ref = serde_json::from_str(&l?)?;
        let ob = &raw[r.ri].obs[r.oi];
        count += 1;
        nc += r.candidates.len();
        if count % 500 == 0 {
            eprintln!("{}", json!({"checked":count,"int8":precision=="int8"}));
        }
        for (hi, p) in [&r.s32, &r.s61].iter().enumerate() {
            let s = model.score(&p.context, &r.candidates, &mut ws, false)?;
            if s.len() != p.scores.len() {
                return Err("reference shape".into());
            }
            for (a, b) in s.iter().zip(&p.scores) {
                if !a.is_finite() {
                    return Err("nonfinite score".into());
                }
                max[hi] = max[hi].max((a - b).abs());
            }
            let rank = order(ob, &s);
            if rank[..rank.len().min(9)] == p.top9 {
                same[hi] += 1;
            }
        }
    }
    if count != 2000 {
        return Err("reference count".into());
    }
    Ok(
        json!({"precision":precision,"observations":count,"candidates":nc,"S32":{"max_abs":max[0],"top9_same":same[0],"top9_ratio":same[0] as f64/count as f64},"S61":{"max_abs":max[1],"top9_same":same[1],"top9_ratio":same[1] as f64/count as f64},"pass":if precision=="f32" {Some(max.iter().all(|x|*x<=1e-3)&&same.iter().all(|x|*x==count))} else {None}}),
    )
}
fn scores(
    model: &Lm,
    raw: &[Row],
    v: &HashMap<char, u32>,
    precision: &str,
    h: usize,
) -> Result<Value> {
    let path = format!("data/autocomp/aclm-rust-{precision}-S{h}.jsonl");
    let mut out = BufWriter::new(File::create(&path)?);
    let mut n = 0;
    let mut count = 0;
    // Five compute workers plus this writer: at most six active CPU jobs.
    std::thread::scope(|scope| -> Result<()> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<
            std::result::Result<(usize, usize, Vec<Vec<f64>>), String>,
        >(10);
        for worker in 0..5 {
            let tx = tx.clone();
            scope.spawn(move || {
                let mut ws = model.workspace(60);
                for ri in (worker..raw.len()).step_by(5) {
                    for (oi, ob) in raw[ri].obs.iter().enumerate() {
                        let (context, adds) = tokenize(ob, v, h);
                        let s = match model.score(&context, &adds, &mut ws, false) {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = tx.send(Err(e.into()));
                                return;
                            }
                        };
                        let mut at = 0;
                        let nested = ob
                            .lists
                            .iter()
                            .map(|l| {
                                let a = at;
                                at += l.c.len();
                                s[a..at].to_vec()
                            })
                            .collect();
                        if tx.send(Ok((ri, oi, nested))).is_err() {
                            return;
                        }
                    }
                }
            });
        }
        drop(tx);
        for rec in rx {
            let (ri, oi, nested) = rec.map_err(std::io::Error::other)?;
            count += nested.iter().map(Vec::len).sum::<usize>();
            n += 1;
            serde_json::to_writer(&mut out, &json!({"ri":ri,"oi":oi,"scores":nested}))?;
            out.write_all(b"\n")?;
            if n % 10000 == 0 {
                eprintln!(
                    "{}",
                    json!({"h":h,"int8":precision=="int8","observations":n})
                );
            }
        }
        Ok(())
    })?;
    out.flush()?;
    Ok(json!({"h":h,"observations":n,"candidates":count}))
}
fn run_one(
    model: &Lm,
    ob: &Obs,
    v: &HashMap<char, u32>,
    h: usize,
    ws: &mut Workspace,
) -> Result<f64> {
    let t = Instant::now();
    let (context, adds) = tokenize(ob, v, h);
    let s = model.score(&context, &adds, ws, false)?;
    let rank = order(ob, &s);
    std::hint::black_box(rank);
    Ok(t.elapsed().as_secs_f64() * 1000.0)
}
fn quant(a: &[f64], p: f64) -> f64 {
    a[((a.len() as f64 * p).ceil() as usize).max(1) - 1]
}
fn bench(model: &Lm, raw: &[Row], v: &HashMap<char, u32>, h: usize) -> Result<Value> {
    let manifest: Value =
        serde_json::from_reader(File::open("data/autocomp/aclist-lat-manifest.json")?)?;
    let bykey: HashMap<&str, &Row> = raw.iter().map(|r| (r.key.as_str(), r)).collect();
    let mut seen = HashSet::new();
    let mut samples = Vec::new();
    for u in manifest["units"].as_array().ok_or("manifest units")? {
        let k = u["key"].as_str().ok_or("manifest key")?;
        if !seen.insert(k) {
            return Err("duplicate manifest unit".into());
        }
        let r = bykey.get(k).ok_or("manifest unit missing")?;
        samples.push(r.obs.last().ok_or("empty observations")?);
    }
    if samples.len() != 600 {
        return Err("manifest count".into());
    }
    let cold_start = Instant::now();
    let mut ws = model.workspace(60);
    run_one(model, samples[0], v, h, &mut ws)?;
    let cold = cold_start.elapsed().as_secs_f64() * 1000.0;
    for i in 0..100 {
        run_one(model, samples[i % samples.len()], v, h, &mut ws)?;
    }
    let mut rounds = Vec::new();
    let mut worst = 0.0f64;
    let mut max_mem = ws.memory_bytes();
    for _ in 0..3 {
        let mut times = Vec::new();
        let (mut trunk, mut output, mut total) = (0.0, 0.0, 0.0);
        for ob in &samples {
            let t = run_one(model, ob, v, h, &mut ws)?;
            times.push(t);
            total += t;
            trunk += ws.timing.trunk_ms;
            output += ws.timing.output_ms;
            max_mem = max_mem.max(ws.memory_bytes());
        }
        times.sort_by(f64::total_cmp);
        let p99 = quant(&times, 0.99);
        worst = worst.max(p99);
        rounds.push(json!({"p50_ms":quant(&times,0.5),"p95_ms":quant(&times,0.95),"p99_ms":p99,"max_ms":times.last(),"trunk_pct":trunk/total*100.0,"output_pct":output/total*100.0,"other_pct":(total-trunk-output)/total*100.0}));
    }
    Ok(
        json!({"h":h,"samples":samples.len(),"warmup":100,"rounds":rounds,"worst_p99_ms":worst,"first_score_ms":cold,"first_score_scope":"first score after independent model load; includes workspace allocation; OS cache retained","weights_mb":model.weight_bytes() as f64/1e6,"workspace_mb":max_mem as f64/1e6,"memory_unit_bytes":1000000}),
    )
}
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("check");
    let selected = args.get(2).map(String::as_str).unwrap_or("all");
    if !["all", "f32", "int8"].contains(&selected) {
        return Err("precision must be all, f32 or int8".into());
    }
    if mode == "bench" {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessAffinityMask};
            SetProcessAffinityMask(GetCurrentProcess(), 1usize << 31)?;
        }
        #[cfg(not(windows))]
        return Err("bench CPU affinity is implemented for Windows".into());
    }
    let raw = rows("data/autocomp/tune-acdump.jsonl")?;
    let vocab: Vec<String> = serde_json::from_reader(File::open("data/pred/lm-vocab.json")?)?;
    let v: HashMap<char, u32> = vocab
        .iter()
        .enumerate()
        .skip(3)
        .map(|(i, s)| Ok((s.chars().next().ok_or("empty vocab")?, i as u32)))
        .collect::<Result<_>>()?;
    let mut reports = Vec::new();
    let mut passed = true;
    for (precision, int8) in [("f32", false), ("int8", true)] {
        if selected != "all" && selected != precision {
            continue;
        }
        if mode == "bench" {
            let mut measurements = Vec::new();
            for h in [32, 61] {
                let load = Instant::now();
                let model = Lm::load("data/pred/lm-f32.bin", int8)?;
                let load_ms = load.elapsed().as_secs_f64() * 1000.0;
                // No score may precede bench's first score on this model instance.
                let mut measurement = bench(&model, &raw, &v, h)?;
                measurement["load_ms"] = json!(load_ms);
                measurements.push(measurement);
            }
            reports.push(json!({"precision":precision,"int8_backend":if int8 {format!("{:?}",twime_model::qkernel::selected_backend())} else {"none".into()},"bench":measurements}));
            continue;
        }
        let model = Lm::load("data/pred/lm-f32.bin", int8)?;
        let r = match mode {
            "check" => {
                let r = check(&model, &raw, precision)?;
                if !int8 {
                    passed = r["pass"].as_bool().unwrap();
                }
                r
            }
            "scores" => {
                json!({"precision":precision,"scores":[scores(&model,&raw,&v,precision,32)?,scores(&model,&raw,&v,precision,61)?]})
            }
            _ => return Err("mode must be check, scores or bench".into()),
        };
        reports.push(r);
    }
    let suffix = if selected == "all" {
        String::new()
    } else {
        format!("-{selected}")
    };
    report_file(
        &format!("data/autocomp/aclm-{mode}{suffix}.json"),
        &json!(reports),
    )?;
    if !passed {
        return Err("f32 reference check failed".into());
    }
    Ok(())
}
