//! AUTOCOMP-POP tuning: P1 wire fingerprints, P2 fixed-pool targets and actual hints, P5 paired pages.
use super::*;

fn frozen_pool(m: &HashMap<String, String>) -> HashMap<String, HashMap<usize, (String, Vec<twime_model::autocomp::Completion>)>> {
    // P2's target set and coverage denominator come from the original LIST dump, never today's UI pool.
    fs::read_to_string(m.get("pop_acdump").map_or("data/autocomp/tune-acdump.jsonl", String::as_str)).expect("frozen tuning pool").lines().map(|line| {
        let r: serde_json::Value=serde_json::from_str(line).expect("frozen pool row");
        let obs=r["obs"].as_array().unwrap().iter().map(|ob| {
            let mut items=vec![];
            for list in ob["lists"].as_array().unwrap() {
                let k=list["k"].as_u64().unwrap() as usize;
                for entry in list["c"].as_array().unwrap() {
                    let word=entry[0].as_str().unwrap().to_string();
                    let extra=entry[1].as_array().unwrap().iter().map(|s|s.as_str().unwrap().to_string()).collect();
                    items.push(twime_model::autocomp::Completion { extra_chars:word.chars().skip(k).collect(),k,word,extra,score:entry[2].as_f64().unwrap(),user:entry[3].as_bool().unwrap(),share:0. });
                }
            }
            (ob["pos"].as_u64().unwrap() as usize,(ob["h"].as_str().unwrap().to_string(),items))
        }).collect();
        (r["key"].as_str().unwrap().to_string(),obs)
    }).collect()
}

pub(super) fn fingerprint(e: &Arc<Engine>, r: &[String]) -> u64 {
    let mut s = hint_session(e, &r[1]);
    s.set_pop_peer(true);
    fingerprint_session(&mut s, r)
}
pub(super) fn fingerprint_session(s: &mut Session, r: &[String]) -> u64 {
    let mut hash = DefaultHasher::new();
    let mut add = |u: Update| {
        for v in 1..=6 { twime_proto::Reply::Update { session: 1, seq: 1, update: u.clone() }.encode_for(v).hash(&mut hash); }
    };
    for x in syls(&r[2]) { for k in keys_of(&x) { add(press(s, k)); } }
    add(press(s, Key::Down));
    add(press(s, Key::Digit(2)));
    add(press(s, Key::Enter));
    hash.finish()
}
pub(super) fn run(a: &[String], quality: bool) {
    std::panic::set_hook(Box::new(|_| eprintln!("acpop evaluation error (text withheld)")));
    let m = settings(&a[2]);
    let (assets, base) = configured(&m);
    let es = [Engine::new(assets.clone(), Config { autocomp_pop: false, ..base.clone() }), Engine::new(assets.clone(), base)];
    let frozen = quality.then(|| frozen_pool(&m));
    let threads: usize = a[5].parse().unwrap();
    assert!((1..=6).contains(&threads));
    let out = par(rows(&a[3]), threads, move |r| {
        let fp = fingerprint(&es[0], r);
        let same = fp == fingerprint(&es[1], r);
        if !quality { return serde_json::json!({"key":r[0],"off_fp":format!("{fp:016x}"),"same":same}).to_string(); }
        let mut on = hint_session(&es[1], &r[1]); on.set_pop_peer(true);
        let mut off = hint_session(&es[0], &r[1]); off.set_pop_peer(true);
        let syl = syls(&r[2]);
        let gold: Vec<char> = r[3].chars().collect();
        let mut frequency = [0usize; 6];
        let mut v7 = [0usize; 8]; // full targets, capped targets, raw hits, hint hits, full nonempty, shown in full, fixed20 raw hits, fixed20 hint hits
        let mut counts = [0usize; 14]; // obs, raw nonempty, targets, raw hit, hints, actual hit, off page, on page, hint off page, hint on page, suppressed, empty, syllables, shown within frozen pool
        let frozen_obs = &frozen.as_ref().unwrap()[&r[0]];
        for (i, x) in syl.iter().enumerate() {
            let mut last = Update::default();
            for k in keys_of(x) { press(&mut off,k); last=press(&mut on,k); }
            let pos = i+1;
            let correct = |c: &twime_model::autocomp::Completion| {
                let end = pos+c.extra.len();
                c.k <= pos && end <= syl.len() && gold.get(pos-c.k..end).is_some_and(|g| g.iter().copied().eq(c.word.chars())) && syl[pos..end] == c.extra
            };
            let point = frozen_obs.get(&pos);
            let raw = point.map(|(_, c)|c.clone()).unwrap_or_default();
            let target = raw.iter().any(correct);
            let full = on.autocomp_lists_now(usize::MAX);
            let full_nonempty = full.iter().any(|(_, l)| !l.is_empty());
            let full_target = full.iter().flat_map(|(_, l)| l).any(correct);
            let current = on.completion_raw_pool();
            let current_target = current.iter().any(correct);
            let h = point.map(|(h, _)| h.clone()).unwrap_or_default();
            let ranked = assets.completion_lm.as_ref().unwrap().rank(&h, current).expect("rank current");
            let raw_hit = ranked.iter().take(9).any(correct);
            v7[0] += usize::from(full_target); v7[1] += usize::from(current_target);
            v7[2] += usize::from(full_target && raw_hit); v7[4] += usize::from(full_nonempty);
            v7[6] += usize::from(target && raw_hit);
            counts[0]+=1; counts[1]+=usize::from(!raw.is_empty()); counts[2]+=usize::from(target);
            if !raw.is_empty() {
                let h = &point.unwrap().0;
                let ranked = assets.completion_lm.as_ref().unwrap().rank(&h, raw).expect("rank");
                counts[3]+=usize::from(target && ranked.iter().take(9).any(correct));
            }
            let seq = 100000+pos as u32*2;
            let started = std::time::Instant::now();
            let hint = on.handle(seq, None, Event::Idle(last.pop_gen)).completion;
            let visible_ms = 250.0 + started.elapsed().as_secs_f64()*1000.;
            for (j, interval) in [100.,150.,200.,250.,300.,400.].iter().enumerate() {
                if hint.is_some() && (pos == syl.len() || visible_ms < *interval) { frequency[j]+=1; }
            }
            let shown = hint.is_some();
            counts[4]+=usize::from(shown);
            counts[13]+=usize::from(shown && point.is_some());
            v7[5] += usize::from(shown && full_nonempty);
            if full_target && shown {
                // Match the recorded result's full identity; suffix-only display is intentionally insufficient.
                let snapshot = on.prepare_idle(seq+1, None, last.pop_gen).map(|s| s.compute()).unwrap_or_default();
                let hit = snapshot.iter().any(correct);
                counts[5]+=usize::from(target && hit);
                v7[3]+=usize::from(hit);
                v7[7]+=usize::from(target && hit);
            }
            if last.hint.is_some() { counts[10]+=1; }
            else if !shown { counts[11]+=1; }
            press(&mut off, Key::Down);
            if let Some(h) = hint { on.handle(seq+2,None,Event::DownCompletion(h.id)); }
            else { press(&mut on,Key::Down); }
            let lo=off.candidate_identities();
            let ln=on.candidate_identities();
            let good: Vec<_> = lo.iter().filter(|(text,span,_,extra)| extra.is_empty() && *span<=pos && text.chars().count()==*span
                && gold.get(pos-span..pos).is_some_and(|g| g.iter().copied().eq(text.chars()))).collect();
            let ho=lo.iter().take(9).any(|it|good.contains(&it));
            let hn=ln.iter().take(9).any(|it|good.contains(&it));
            counts[6]+=usize::from(ho); counts[7]+=usize::from(hn);
            if shown { counts[8]+=usize::from(ho); counts[9]+=usize::from(hn); }
            press(&mut off, Key::Esc); press(&mut on, Key::Esc);
        }
        counts[12]=syl.len();
        serde_json::json!({"key":r[0],"off_fp":format!("{fp:016x}"),"same":same,"counts":counts,"v7":v7,"frequency":frequency}).to_string()
    });
    write(&a[4], &out);
    println!("acpop rows={} quality={quality}", out.len());
}

pub(super) fn expected(a: &[String]) {
    let m=settings(&a[2]);
    let (assets,config)=configured(&m);
    let e=Engine::new(assets,config);
    let manifest: serde_json::Value=serde_json::from_slice(&fs::read(&a[3]).unwrap()).unwrap();
    let mut out=vec![];
    for u in manifest["units"].as_array().unwrap() {
        let mut s=hint_session(&e,u["context"].as_str().unwrap()); s.set_pop_peer(true);
        let mut last=Update::default();
        for x in u["syllables"].as_array().unwrap() { for k in keys_of(x.as_str().unwrap()) { last=press(&mut s,k); } }
        let options=s.handle(10000,None,Event::Idle(last.pop_gen)).completion.map(|h|h.options).unwrap_or_default();
        out.push(options);
    }
    fs::write(&a[4],serde_json::to_vec(&out).unwrap()).unwrap();
    println!("acpop expected={}",out.len());
}

fn configured(m: &HashMap<String, String>) -> (Arc<Assets>, Config) {
    let mut assets=load(m);
    assets.completion_lm=twime_model::pop::CompletionLm::load_optional(std::path::Path::new(m.get("pop_lm_dir").map_or("data/pred",String::as_str))).expect("LM");
    assert!(assets.completion_lm.is_some(), "POP requires LM assets");
    (Arc::new(assets), Config { lambda:m.get("lambda").map_or(0.5,|v|v.parse().unwrap()), autocomp_list:true, autocomp_pop:true,
        lexux:twime_model::lexux::Config::parse(m).expect("lexux keys"), ..Config::default() })
}

pub(super) fn show(a: &[String]) {
    let m=settings(&a[2]);
    let (assets,config)=configured(&m);
    let e=Engine::new(assets,config);
    let mut s=hint_session(&e,&a[3]); s.set_pop_peer(true);
    let mut last=Update::default();
    for x in &a[4..] { for syllable in syls(x) { for k in keys_of(&syllable) { last=press(&mut s,k); } } }
    let options=s.handle(10000,None,Event::Idle(last.pop_gen)).completion.map(|h|h.options).unwrap_or_default();
    println!("{}",serde_json::json!({"count":options.len(),"options":options}));
}
