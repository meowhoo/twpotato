use super::*;

pub(crate) fn model(vocab: usize) -> N1 {
    let mut m = N1::random_g1(vocab, 64, 3, 97, 731);
    m.prefix = true;
    m
}

pub(crate) fn entries(m: &N1) -> Vec<usize> {
    m.prefix_cache.lock().unwrap().iter().map(|e| Arc::as_ptr(e) as usize).collect()
}

fn compare(a: &[Vec<f32>], b: &[Vec<f32>], tol: f32) -> (usize, usize) {
    assert_eq!(a.len(), b.len());
    let (mut same, mut total) = (0, 0);
    for (a, b) in a.iter().zip(b) {
        assert_eq!(a.len(), b.len());
        for (&x, &y) in a.iter().zip(b) {
            assert!((x-y).abs() <= tol, "{x} vs {y}");
            same += usize::from(x.to_bits() == y.to_bits());
            total += 1;
        }
    }
    (same, total)
}

fn sequences() -> Vec<(Vec<u32>, usize)> {
    let mut cases = Vec::new();
    let mut add = |prefix: &[u32], suffix: &[u32]| {
        let mut tokens = prefix.to_vec();
        tokens.push(SEP);
        let first = tokens.len();
        tokens.extend_from_slice(suffix);
        cases.push((tokens, first));
    };
    for s in [&[][..], &[9][..], &[9,10][..], &[9,10,11][..], &[9,10][..], &[9,12,10][..]] {
        add(&[3,4], s);
    }
    for p in [&[5,6][..], &[3,4][..], &[5,6][..], &[3,4][..]] { add(p, &[9,10]); }
    for p in 3..8 { add(&[p], &[9,10,11]); }
    add(&[3], &[9,10]); // first of the five was evicted
    add(&[], &[9,10]);
    add(&[], &[9,10,11]);
    add(&[4;64], &[9,10]);
    add(&[4;64], &[9,10,11]);
    cases
}

#[test]
fn fixture_prefix_scores() {
    let (m, exp) = super::tests::g2_fixture("g1fc");
    assert!(m.prefix);
    let ids = |v: &serde_json::Value| -> Vec<u32> {
        v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect()
    };
    let mut max = 0.0f64;
    for (k, seq) in exp["seqs"].as_array().unwrap().iter().enumerate() {
        let tokens = ids(seq);
        let first = exp["firsts"][k].as_u64().unwrap() as usize;
        let legal: Vec<_> = exp["legal"][k].as_array().unwrap().iter().map(ids).collect();
        let refs: Vec<_> = legal.iter().map(Vec::as_slice).collect();
        m.set_prefix_cache(false);
        let full = m.scores(&tokens, first, &refs).unwrap();
        m.set_prefix_cache(true);
        for _ in 0..2 { // miss and hit
            let got = m.scores(&tokens, first, &refs).unwrap();
            compare(&full, &got, 1e-5);
            for (i, row) in got.iter().enumerate() {
                for (j, &v) in row.iter().enumerate() {
                    max = max.max((v as f64-exp["scores"][k][i][j].as_f64().unwrap()).abs());
                }
            }
        }
        for (i, row) in full.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                max = max.max((v as f64-exp["scores"][k][i][j].as_f64().unwrap()).abs());
            }
        }
    }
    assert!(max <= 1e-4, "{max}");
    println!("g1fc fixture_max_abs_error={max:.10e}");
    let _lock = crate::qkernel::BACKEND_TEST_LOCK.lock().unwrap();
    let (mut q, _) = super::tests::g2_fixture("g1fc");
    q.quantize_int8().unwrap();
    for (k, seq) in exp["seqs"].as_array().unwrap().iter().enumerate() {
        let tokens = ids(seq);
        let first = exp["firsts"][k].as_u64().unwrap() as usize;
        let legal: Vec<_> = exp["legal"][k].as_array().unwrap().iter().map(ids).collect();
        let refs: Vec<_> = legal.iter().map(Vec::as_slice).collect();
        q.set_prefix_cache(false);
        let full = q.scores(&tokens, first, &refs).unwrap();
        let pred = q.predict(&tokens, first, &refs);
        q.set_prefix_cache(true);
        for _ in 0..2 {
            compare(&q.scores(&tokens, first, &refs).unwrap(), &full, 1e-5);
            assert_eq!(q.predict(&tokens, first, &refs), pred);
        }
    }
}

#[test]
fn editing_equivalence_fp32_int8_and_lru() {
    let _lock = crate::qkernel::BACKEND_TEST_LOCK.lock().unwrap();
    for int8 in [false, true] {
        let mut m = model(24);
        let mut reference = model(24);
        // Two identical output rows exercise the smallest-id tie rule.
        for x in [&mut m, &mut reference] {
            let row = x.tok[7*x.d..8*x.d].to_vec();
            x.tok[8*x.d..9*x.d].copy_from_slice(&row);
            if int8 { x.quantize_int8().unwrap(); }
        }
        reference.set_prefix_cache(false);
        let (mut same, mut total) = (0,0);
        for (t, first) in sequences() {
            let legal = vec![&[3,4,7,8][..]; t.len()-first];
            let want = reference.scores(&t, first, &legal).unwrap();
            for _ in 0..2 {
                let got = m.scores(&t, first, &legal).unwrap();
                let (s,n) = compare(&got, &want, 1e-5);
                same += s; total += n;
                assert_eq!(m.predict(&t,first,&legal), reference.predict(&t,first,&legal));
                let ties = vec![&[7,8][..]; legal.len()];
                assert_eq!(m.predict(&t,first,&ties).unwrap(), vec![Some(7);legal.len()]);
            }
            assert!(entries(&m).len() <= 4);
        }
        println!("prefix int8={int8} bit_identical={same}/{total} ({:.3}%)", same as f64*100.0/total as f64);
        // Explicit LRU assertions, including recency updates and actual rebuilding.
        m.prefix_cache.lock().unwrap().clear();
        let score = |p| m.scores(&[p,SEP,9],2,&[&[3,4]]).unwrap();
        score(3);
        let first_entry = m.prefix_cache.lock().unwrap()[0].clone();
        for p in 4..7 { score(p); }
        score(3);
        score(7);
        {
            let c = m.prefix_cache.lock().unwrap();
            assert_eq!(c.len(),4);
            assert!(c.iter().any(|e| Arc::ptr_eq(e,&first_entry)));
            assert!(!c.iter().any(|e| e.tokens == [4,SEP]));
        }
        for p in 8..12 { score(p); }
        score(3);
        assert!(!m.prefix_cache.lock().unwrap().iter().any(|e| Arc::ptr_eq(e,&first_entry)));
    }
}

#[test]
fn prefix_hidden_independent_of_syllables() {
    for prefix in [false,true] {
        let mut m = model(24);
        m.prefix = prefix;
        let a = m.hidden(&[3,4,SEP,9,10]).unwrap();
        let b = m.hidden(&[3,4,SEP,11,12]).unwrap();
        let a: Vec<_> = a[..3*m.d].iter().map(|x| x.to_bits()).collect();
        let b: Vec<_> = b[..3*m.d].iter().map(|x| x.to_bits()).collect();
        assert_eq!(a == b, prefix);
        if prefix {
            let short = m.hidden(&[3,4,SEP,9]).unwrap();
            assert_eq!(a, short[..3*m.d].iter().map(|x| x.to_bits()).collect::<Vec<_>>());
        }
        // No SEP means fully bidirectional, including for prefix models.
        let a = m.hidden(&[3,4,9]).unwrap();
        let b = m.hidden(&[3,4,10]).unwrap();
        assert_ne!(&a[..m.d], &b[..m.d]);
    }
}

#[test]
fn shared_model_threads() {
    let _lock = crate::qkernel::BACKEND_TEST_LOCK.lock().unwrap();
    for int8 in [false,true] {
        let mut m = model(24);
        let mut reference = model(24);
        if int8 { m.quantize_int8().unwrap(); reference.quantize_int8().unwrap(); }
        reference.set_prefix_cache(false);
        std::thread::scope(|scope| {
            for thread in 0..4 {
                let (m,r) = (&m,&reference);
                scope.spawn(move || {
                    let cases = sequences();
                    for i in 0..cases.len()*2 {
                        let (t, first) = &cases[(i+thread*3)%cases.len()];
                        let legal = vec![&[3,4,7,8][..]; t.len()-first];
                        compare(&m.scores(t,*first,&legal).unwrap(), &r.scores(t,*first,&legal).unwrap(),1e-5);
                        assert_eq!(m.predict(t,*first,&legal),r.predict(t,*first,&legal));
                    }
                });
            }
        });
        assert_eq!(entries(&m).len(),4);
    }
}

#[test]
fn quantization_invalidates_prefix() {
    let _lock = crate::qkernel::BACKEND_TEST_LOCK.lock().unwrap();
    let mut m = model(24);
    let t = [3,4,SEP,9,10];
    let legal: &[&[u32]] = &[&[3,4],&[7,8]];
    m.scores(&t,3,legal).unwrap();
    assert_eq!(entries(&m).len(),1);
    m.quantize_int8().unwrap();
    assert!(entries(&m).is_empty());
    let miss = m.scores(&t,3,legal).unwrap();
    let hit = m.scores(&t,3,legal).unwrap();
    m.set_prefix_cache(false);
    let full = m.scores(&t,3,legal).unwrap();
    compare(&miss,&full,1e-5);
    compare(&hit,&full,1e-5);
}

#[test]
fn cand_explicit_mask_and_no_cache_access() {
    let _lock = crate::qkernel::BACKEND_TEST_LOCK.lock().unwrap();
    for int8 in [false,true] {
        let mut m = model(24);
        if int8 { m.quantize_int8().unwrap(); }
        for p in 3..7 { m.scores(&[p,SEP,9],2,&[&[3,4]]).unwrap(); }
        let before = entries(&m);
        let mut t = vec![3,SEP];
        t.extend((0..109).map(|i| 9+i%3));
        // Explicit first differs from inferred first, ensuring scores_ext forwards it.
        let pos: Vec<u32> = (0..t.len()).map(|i| (i%17) as u32).collect();
        let extra: Vec<f32> = (0..t.len()*m.d).map(|i| (i as f32*0.7).sin()*0.01).collect();
        let legal: &[&[u32]] = &[&[3,4,7,8],&[4,5]];
        // first=2 matches the oldest entry: even a cache lookup would reorder it.
        for first in [2,4] {
            let h = m.hidden_g1_at(&t,Some((&pos,&extra)),first,None,None);
            let expected: Vec<Vec<f32>> = legal.iter().enumerate().map(|(i,ids)| ids.iter().map(|&id| {
                h[(first+i)*m.d..(first+i+1)*m.d].iter().zip(&m.tok[id as usize*m.d..(id as usize+1)*m.d]).map(|(a,b)| a*b).sum()
            }).collect()).collect();
            for on in [true,false] {
                m.set_prefix_cache(on);
                compare(&m.scores_ext(&t,&pos,&extra,111,first,legal).unwrap(),&expected,1e-5);
                assert_eq!(entries(&m),before);
            }
        }
        // The ordinary hidden_ext entry also infers SEP correctly.
        let inferred = m.hidden_ext(&t,&pos,&extra,111).unwrap();
        let explicit = m.hidden_g1_at(&t,Some((&pos,&extra)),2,None,None);
        assert_eq!(inferred,explicit);
        assert_ne!(m.hidden_g1_at(&t,Some((&pos,&extra)),4,None,None),inferred);
    }
}

#[test]
fn prefix_scores_local_timing() {
    let _lock = crate::qkernel::BACKEND_TEST_LOCK.lock().unwrap();
    for int8 in [false,true] {
        let mut m = N1::random_g1(128,128,4,256,731);
        m.prefix = true;
        if int8 { m.quantize_int8().unwrap(); }
        let mut t = vec![3;64]; t.push(SEP); t.extend([9;16]);
        let legal = vec![&[3,4,7,8][..];16];
        m.scores(&t,65,&legal).unwrap();
        for on in [false,true] {
            m.set_prefix_cache(on);
            for _ in 0..5 { std::hint::black_box(m.scores(&t,65,&legal)); }
            let start = std::time::Instant::now();
            for _ in 0..40 { std::hint::black_box(m.scores(&t,65,&legal)); }
            println!("prefix timing int8={int8} cache={on} scores_us={:.2}",start.elapsed().as_secs_f64()*1e6/40.0);
        }
    }
}
