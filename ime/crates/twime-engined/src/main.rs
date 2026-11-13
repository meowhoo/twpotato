//! twime-engined: command line.
//!
//! --assets-dir DIR      data.txt, common-readings.txt, model.bin, prior.pack inside DIR (or the four flags below); also
//!                       DIR\pn.cfg when present and no --pn* flag is given: PN settings `lexicon=BASE` (relative to DIR),
//!                       `b2`, `b3`, `h`, `cands` (format of data/pn/cfg/*.cfg; written by ime/scripts/make-assets.sh)
//! --data-txt F --aliases F --model F [--prior F]
//! --no-model            dictionary only (single-character fallback)
//! --int8 / --no-int8    enable / disable G1 W8A8 inference (last flag wins; default off); conversion errors stop startup
//!                       With --assets-dir and neither flag, DIR\model.cfg applies when present: int8=on|off is
//!                       required. Changes during loading stop startup. Either flag skips its int8 key.
//!                       model.cfg may also carry user_force=on|off and user_bonus2=N (USERLEX), read independently
//!                       of the int8 flags; non-default rules are printed as `userlex force=.. bonus2=..` after the ready line.
//!                       SHORT-PRIOR keys lambda_short_c, lambda_short0, word_short_bonus, prior_ctx (read the same way;
//!                       absent = off) are printed as `shortprior ..` after the ready line when set.
//! --user-force on|off --user-bonus2 N   USERLEX rules (USERLEX-plan 1.1, 1.3); default off / 10
//! --lambda X            prior weight (default 0.5; 0 = no prior)
//! --candidates          let Space / Down open the candidate list (the shell must be able to show it)
//! --no-word-candidates single-character candidates only (word candidates are on by default)
//! --word-beta X        weight of the same-reading rarity penalty of word candidates (default 0)
//! --default-english     version 3 sessions start in English mode (default: Chinese)
//! --punct-half          version 3 sessions start with half-width punctuation style (default: full-width)
//! --no-norm-mo         feed the model ㄇㄛ˙ as it is (default: as ㄇㄜ˙, NORM-MO-plan)
//! --no-norm            turn off the glyph normalization (NORM plan: 臺->台 output, 佈 word rules, 台/臺 style and pairs, context fold)
//! --no-symbol-table     the backquote key types a backquote instead of opening the small symbol table
//! --pn BASE             proper-name lexicon BASE.fst / BASE.lst (PN-plan; off when absent)
//! --pn-b2 X --pn-b3 X   PN bonus of 2-character / longer words (off or a number; default off = no jumps of that length)
//! --pn-h N              PN hits kept per (start, length) (inf or a count; default inf)
//! --pn-no-cands         PN words only in the automatic conversion, not in the candidate list
//! --no-ta-default       keep 她/它/牠/祂 of the automatic conversion (default: read ㄊㄚ they become 他, TA-plan)
//! --cache-gamma X --cache-window N --cache-cap K --cache-source doc|confirmed
//!                       same-document bigram cache (CACHE-plan; off by default); also the keys `cache_gamma`,
//!                       `cache_window`, `cache_cap`, `cache_source` of DIR\pn.cfg when no --cache* flag is given. Only
//!                       used with PN on. `cache_source=confirmed` (CACHE-c): only pairs touching characters chosen in the
//!                       candidate window during this focus (CACHE-plan 7th edition).
//! --genj BASE           SRCH-JUMP BASE.{fst,lst,tier,cfg}; defaults to DIR/genj, complete absence = off
//! --gen BASE            general-word layer BASE.fst / BASE.lst (SRCH-plan 4, mode 0: candidate list only; defaults to DIR\\gen when both files exist)
//! --cand BASE           CAND candidate-aware model (CAND-plan): BASE.clex / BASE.cemb with the model; also
//!                       DIR\cand.cfg when present and no --cand* flag is given: `base=NAME` (relative to DIR), `m=M`
//! --cand-m M            CAND candidate token cap (default: the exported M)
//! --version             print the version and capabilities (`caps=cand-cfg`) and exit
//! --cpu N               pin the process to logical CPU N
//! --pipe NAME           override the pipe name (tools); on Unix a socket path, or a bare name inside the runtime directory
//! --parallel            do not serialize inference across connections
//! --test-events         accept test-only events          --test-faults   enable fault injection
//! --user-data           enable user settings and lexicon in the default user directory
//! --user-dir D          enable user data in D; user-data.on next to this executable also enables the default
//! --data-dir D          state directory (default %LOCALAPPDATA%\TwPotato): running.lock and unconfirmed.json

use std::{io::Write, path::PathBuf, sync::Arc, time::Instant};

use twime_core::{Config, Engine, UserLink};
use twime_engined::{Options, runlock::RunLock, serve};
use twime_model::{Assets, ModelPaths};
use twime_winpipe::PipeServer;

fn main() {
    let mut a = std::env::args().skip(1);
    let (mut dir, mut data, mut aliases, mut model, mut prior): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) =
        (None, None, None, None, None);
    let (mut no_model, mut lambda, mut cands, mut cpu, mut pipe) = (false, 0.5f64, false, None::<u32>, None::<String>);
    let (mut words, mut beta) = (true, Config::default().beta);
    let (mut default_english, mut punct_half, mut symbol_table) = (false, false, true);
    let mut norm = true;
    let mut norm_mo = true;
    let mut int8_flag = None;
    let (mut user_force_flag, mut user_bonus2_flag): (Option<bool>, Option<f64>) = (None, None);
    // FUZZY F1: --repair off|auto|suggest wins over model.cfg's repair key; absent everywhere = off.
    let mut repair_flag: Option<Option<twime_core::RepairMode>> = None;
    // FUZZY F2: --fuzzy-words on|off wins over model.cfg's fuzzy_words key; absent everywhere = off.
    let mut fuzzy_flag: Option<bool> = None;
    // FUZZY F2 step 2: --fuzzy-hints on|off wins over model.cfg's fuzzy_hints key; absent everywhere = off.
    let mut fuzzy_hints_flag: Option<bool> = None;
    // AUTOCOMP: --autocomp on|off wins over model.cfg's autocomp key; absent everywhere = off.
    let mut autocomp_flag: Option<bool> = None;
    // AUTOCOMP-LIST: --autocomp-list on|off wins over model.cfg's autocomp_list key; absent everywhere = off.
    let mut autocomp_list_flag: Option<bool> = None;
    let mut autocomp_pop_flag: Option<bool> = None;
    // EMOJI: --emoji on|off wins over model.cfg's emoji key; absent everywhere = off.
    let mut emoji_flag: Option<bool> = None;
    // TA (TA-plan): ㄊㄚ variants of the automatic conversion become 他; --no-ta-default switches it off (rollback).
    let mut ta_default = true;
    let (mut pn_base, mut pn_cfg) = (None::<PathBuf>, twime_model::pn::PnCfg::default());
    let mut pn_flags = false;
    let (mut cache_keys, mut cache_flags) = (std::collections::HashMap::<String, String>::new(), false);
    // pn.cfg of the assets directory and its modification time (None = absent) when read; checked again after loading.
    let mut pn_file: Option<(PathBuf, Option<std::time::SystemTime>)> = None;
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|x| x.modified()).ok();
    let mut gen_base = None::<PathBuf>;
    let mut genj_base = None::<PathBuf>;
    let mut measure_touch = None::<PathBuf>;
    let mut measure_done = None::<PathBuf>;
    let (mut cand_base, mut cand_m, mut cand_flags) = (None::<PathBuf>, None::<usize>, false);
    let mut cand_file: Option<(PathBuf, Option<std::time::SystemTime>)> = None;
    let mut opts = Options { serial: true, ..Options::default() };
    let mut data_dir: Option<PathBuf> = None;
    let mut user_data = false;
    let mut user_dir: Option<PathBuf> = None;
    while let Some(x) = a.next() {
        let mut val = || a.next().unwrap_or_else(|| usage(&format!("{x} needs a value")));
        pn_flags |= x.starts_with("--pn");
        cand_flags |= x.starts_with("--cand");
        cache_flags |= x.starts_with("--cache");
        match x.as_str() {
            "--assets-dir" => dir = Some(val().into()),
            "--data-txt" => data = Some(val().into()),
            "--aliases" => aliases = Some(val().into()),
            "--model" => model = Some(val().into()),
            "--prior" => prior = Some(val().into()),
            "--no-model" => no_model = true,
            "--lambda" => lambda = val().parse().unwrap_or_else(|_| usage("bad --lambda")),
            "--candidates" => cands = true,
            "--no-word-candidates" => words = false,
            "--default-english" => default_english = true,
            "--punct-half" => punct_half = true,
            "--no-symbol-table" => symbol_table = false,
            "--no-norm" => norm = false,
            "--no-norm-mo" => norm_mo = false,
            "--pn" => pn_base = Some(val().into()),
            "--genj" => genj_base = Some(val().into()),
            "--gen" => gen_base = Some(val().into()),
            "--cand" => cand_base = Some(val().into()),
            "--cand-m" => cand_m = Some(val().parse().unwrap_or_else(|_| usage("bad --cand-m"))),
            "--version" => {
                println!("twime-engined {} caps=cand-cfg", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--pn-b2" => pn_cfg.b2 = twime_model::pn::parse_bonus(&val()).unwrap_or_else(|_| usage("bad --pn-b2")),
            "--pn-b3" => pn_cfg.b3 = twime_model::pn::parse_bonus(&val()).unwrap_or_else(|_| usage("bad --pn-b3")),
            "--pn-h" => pn_cfg.h = twime_model::pn::parse_h(&val()).unwrap_or_else(|_| usage("bad --pn-h")),
            "--pn-no-cands" => pn_cfg.cands = false,
            "--cache-gamma" => drop(cache_keys.insert("cache_gamma".into(), val())),
            "--cache-window" => drop(cache_keys.insert("cache_window".into(), val())),
            "--cache-cap" => drop(cache_keys.insert("cache_cap".into(), val())),
            "--cache-source" => drop(cache_keys.insert("cache_source".into(), val())),
            "--user-force" => user_force_flag = Some(match val().as_str() { "on" => true, "off" => false, _ => usage("bad --user-force") }),
            "--user-bonus2" => user_bonus2_flag = Some(val().parse::<f64>().ok().filter(|b| b.is_finite() && (0.0..=30.0).contains(b)).unwrap_or_else(|| usage("bad --user-bonus2"))),
            "--repair" => repair_flag = Some(twime_engined::parse_repair(&val()).unwrap_or_else(|| usage("bad --repair"))),
            "--autocomp-pop" => autocomp_pop_flag = Some(match val().as_str() { "on" => true, "off" => false, _ => usage("bad --autocomp-pop") }),
            "--emoji" => emoji_flag = Some(match val().as_str() { "on" => true, "off" => false, _ => usage("bad --emoji") }),
            "--autocomp-list" => autocomp_list_flag = Some(match val().as_str() { "on" => true, "off" => false, _ => usage("bad --autocomp-list") }),
            "--autocomp" => autocomp_flag = Some(match val().as_str() { "on" => true, "off" => false, _ => usage("bad --autocomp") }),
            "--fuzzy-hints" => fuzzy_hints_flag = Some(match val().as_str() { "on" => true, "off" => false, _ => usage("bad --fuzzy-hints") }),
            "--fuzzy-words" => fuzzy_flag = Some(match val().as_str() { "on" => true, "off" => false, _ => usage("bad --fuzzy-words") }),
            "--int8" => int8_flag = Some(true),
            "--no-int8" => int8_flag = Some(false),
            "--no-ta-default" => ta_default = false,
            "--word-beta" => beta = val().parse().unwrap_or_else(|_| usage("bad --word-beta")),
            "--cpu" => cpu = Some(val().parse().unwrap_or_else(|_| usage("bad --cpu"))),
            "--pipe" => pipe = Some(val()),
            "--data-dir" => data_dir = Some(val().into()),
            "--user-data" => user_data = true,
            "--user-dir" => user_dir = Some(val().into()),
            "--parallel" => opts.serial = false,
            "--test-events" => opts.allow_test_events = true,
            "--lexux-measure-touch" => measure_touch = Some(val().into()),
            "--lexux-measure-done" => measure_done = Some(val().into()),
            "--test-faults" => opts.test_faults = true,
            _ => usage(&format!("unknown argument {x}")),
        }
    }
    // Windows: integrity levels. Unix: not root, real user = effective user; the socket is private to the user.
    #[cfg(windows)]
    let refusal = match twime_winpipe::current_token_security() {
        Ok(token) => engine_token_refusal(token.integrity_rid, &token.user).map(str::to_owned),
        Err(e) => Some(format!("cannot query process token: {e}")),
    };
    #[cfg(not(windows))]
    let refusal = twime_winpipe::privilege_refusal().map(str::to_owned).or_else(|| {
        // The state and user directories fall back to HOME: without an absolute one they would land in the working directory.
        let home = std::env::var_os("HOME").map(PathBuf::from).is_some_and(|h| h.is_absolute());
        (!home).then(|| "HOME is not an absolute path".to_owned())
    });
    if let Some(reason) = refusal {
        eprintln!("twime-engined: {reason}");
        std::process::exit(6);
    }
    // Tracked whenever there is an assets directory: int8 and the USERLEX keys may both come from it (USERLEX 1.3).
    let model_mtime = dir.as_ref().and_then(|d| mtime(&d.join("model.cfg")));
    let (int8, _) = twime_engined::int8_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e));
    // USERLEX 1.3: each explicit --user-* flag wins for its own key; otherwise model.cfg; absent = the old behaviour.
    let (file_force, file_bonus2) = twime_engined::user_rules_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e));
    let user_force = user_force_flag.or(file_force).unwrap_or(false);
    let user_bonus2 = user_bonus2_flag.or(file_bonus2).unwrap_or(twime_model::user::DEFAULT_BONUS);
    // SHORT-PRIOR stage A: model.cfg keys only (no flags); absent = off.
    let (short, short_table) = twime_engined::short_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e));
    let wordbonus_file = twime_engined::wordbonus_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e));
    let lexux = twime_engined::lexux_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e));
    let repair_mode = match repair_flag {
        Some(m) => m,
        None => twime_engined::repair_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e)),
    };
    let fuzzy_on = match fuzzy_flag {
        Some(f) => f,
        None => twime_engined::fuzzy_words_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e)),
    };
    // SETTINGS-AUTOCOMP: flag > user-level autocomp.cfg (only with user data on; a bad file is ignored whole) > model.cfg > off.
    // model.cfg stays strict: it is read (and may stop the engine) only for a key nothing above overrides.
    let ac_user_data = user_data || user_dir.is_some() || std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("user-data.on").is_file())).unwrap_or(false);
    let ac_loaded = twime_engined::autocomp_user_cfg(ac_user_data, &user_dir.clone().unwrap_or_else(twime_userdata::user_dir));
    if ac_loaded == twime_userdata::autocomp_cfg::Loaded::Bad {
        eprintln!("twime-engined: autocomp.cfg in the user directory is unusable; ignored");
    }
    let ac_user = ac_loaded.effective();
    let autocomp_pop_on = twime_engined::autocomp_pop_effective(autocomp_pop_flag, ac_user.pop, int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e));
    let autocomp_list_on = twime_engined::autocomp_list_effective(autocomp_list_flag, ac_user.list, int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e));
    let emoji_on = match emoji_flag {
        Some(f) => f,
        None => twime_engined::emoji_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e)),
    };
    let autocomp_on = match autocomp_flag {
        Some(f) => f,
        None => twime_engined::autocomp_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e)),
    };
    let fuzzy_hints_on = match fuzzy_hints_flag {
        Some(f) => f,
        None => twime_engined::fuzzy_hints_setting(int8_flag, dir.as_deref()).unwrap_or_else(|e| fail(&e)),
    };
    // Tracked even when absent (like pn.cfg): a model.cfg appearing, vanishing or changing while we load stops startup.
    let model_file = dir.as_ref().map(|d| (d.join("model.cfg"), model_mtime));
    if let Some(d) = &dir {
        data = data.or(Some(d.join("data.txt")));
        aliases = aliases.or(Some(d.join("common-readings.txt")));
        model = model.or(Some(d.join("model.bin")));
        prior = prior.or_else(|| Some(d.join("prior.pack")).filter(|p| p.exists()));
        // PN settings next to the model (PNFIX): the installed engine is started with --assets-dir only.
        let pc = d.join("pn.cfg");
        if !pn_flags {
            pn_file = Some((pc.clone(), mtime(&pc)));
        }
        if !pn_flags && pc.exists() {
            let m = twime_model::pn::read_settings(&pc).unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", pc.display())));
            pn_cfg = twime_model::pn::PnCfg::from_settings(&m).unwrap_or_else(|e| fail(&format!("bad {}: {e}", pc.display())));
            pn_base = Some(d.join(m.get("lexicon").unwrap_or_else(|| fail(&format!("{} has no lexicon", pc.display())))));
        }
        // CACHE plan 2.3: without --cache* flags the cache_* keys of pn.cfg apply, whatever the --pn* flags say.
        if !cache_flags && pc.exists() {
            let m = twime_model::pn::read_settings(&pc).unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", pc.display())));
            cache_keys = m.into_iter().filter(|(k, _)| k.starts_with("cache_")).collect();
            pn_file = Some((pc.clone(), mtime(&pc)));
        }
    }
    // CAND settings next to the model (CAND-plan 5.5), same rules as pn.cfg.
    if let Some(d) = &dir {
        let cc = d.join("cand.cfg");
        if !cand_flags && cc.exists() {
            let mt = mtime(&cc);
            let m = twime_model::pn::read_settings(&cc).unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", cc.display())));
            cand_base = Some(d.join(m.get("base").unwrap_or_else(|| fail(&format!("{} has no base", cc.display())))));
            cand_m = Some(m.get("m").and_then(|x| x.parse().ok()).unwrap_or_else(|| fail(&format!("{} has no valid m", cc.display()))));
            cand_file = Some((cc, mt));
        }
    }
    // An uninstall in progress (marker next to the executable): do not start (N3-plan section 8).
    if std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(twime_proto::ids::UNINSTALL_MARKER))).is_some_and(|m| m.exists()) {
        eprintln!("twime-engined: uninstall marker present, exiting");
        std::process::exit(4);
    }
    // running.lock first (N3-plan section 8): failing here means no connection is ever accepted.
    let logon = match twime_winpipe::logon_session_id() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("twime-engined: cannot read the logon session id: {e}");
            std::process::exit(5);
        }
    };
    let lock = match RunLock::acquire(&data_dir.unwrap_or_else(twime_winpipe::state::data_dir), logon) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("twime-engined: cannot take running.lock (another engine, or the state files cannot be handled): {e}");
            std::process::exit(5);
        }
    };
    if uninstall_after_lock(&lock, std::env::current_exe().ok()) {
        eprintln!("twime-engined: uninstall marker present, exiting");
        std::process::exit(4);
    }
    opts.marker_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    // SRCH mode 0: an explicit --gen wins; a partial assets pair keeps the layer off. Probed only after running.lock and
    // the uninstall marker check, so an installation that finished before this point is seen whole.
    gen_base = gen_base.or_else(|| dir.as_ref().map(|d| d.join("gen"))
        .filter(|b| b.with_extension("fst").is_file() && b.with_extension("lst").is_file()));
    genj_base = genj_base.or_else(|| dir.as_ref().map(|d| d.join("genj")));
    let (Some(data_txt), Some(aliases), Some(model)) = (data, aliases, model) else { usage("assets missing") };
    #[cfg(windows)]
    if let Some(c) = cpu {
        unsafe {
            let _ = windows::Win32::System::Threading::SetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), 1usize << c);
        }
    }
    #[cfg(not(windows))]
    if cpu.is_some() {
        usage("--cpu is Windows only; start the engine under taskset instead");
    }
    let cache_cfg = twime_model::cache::CacheCfg::from_settings(&cache_keys).unwrap_or_else(|e| usage(&format!("cache settings: {e}")));
    let t = Instant::now();
    // FUZZY 3.2: the repair ranking uses data.txt without the alias table (read only when the repair is on)
    let repair_data = (repair_mode.is_some() || fuzzy_on).then(|| std::fs::read_to_string(&data_txt).unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", data_txt.display()))));
    let paths = ModelPaths { data_txt, aliases, model, prior: if lambda == 0.0 { None } else { prior } };
    let assets = if no_model { Assets::load_dictionary_only(&paths) } else { Assets::load(&paths) };
    let mut assets = match assets {
        Ok(mut a) => {
            if int8 { a.set_int8().unwrap_or_else(|e| fail(&format!("int8: {e}"))); }
            if norm {
                a.set_norm(twime_model::NormCfg::ALL);
            }
            a.set_mo_norm(norm_mo);   // NORM-MO: ㄇㄛ˙ is fed to the model as ㄇㄜ˙ (NORM-MO-plan); --no-norm-mo = the old input
            a.set_ta_default(ta_default);
            a.set_user_rules(user_force, user_bonus2).unwrap_or_else(|_| usage("bad user rules"));
            a.set_short(short).unwrap_or_else(|e| fail(&e));
            if let Some(t) = &short_table {
                let m = Assets::load_short_table(t).unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", t.display())));
                a.set_short_table(Some(m));
            }
            if let Some(b) = &pn_base {
                match twime_model::pn::PnLexicon::open(b, &a) {
                    Ok(lex) => a.set_pn(Some(Arc::new(lex)), pn_cfg.clone()),
                    Err(e) => {
                        eprintln!("twime-engined: cannot open the PN lexicon {}: {e}", b.display());
                        std::process::exit(2);
                    }
                }
            }
            if let Some(b) = &cand_base {
                match twime_model::cand::CandCfg::open(b, &a, cand_m) {
                    Ok(c) => a.set_cand(Some(Arc::new(c))),
                    Err(e) => {
                        eprintln!("twime-engined: cannot open the CAND files {}: {e}", b.display());
                        std::process::exit(2);
                    }
                }
            }
            if let Some(b) = &gen_base {
                match twime_model::pn::PnLexicon::open(b, &a) {
                    Ok(lex) => a.set_gen(Some(Arc::new(lex))),
                    Err(e) => {
                        eprintln!("twime-engined: cannot open the general-word lexicon {}: {e}", b.display());
                        std::process::exit(2);
                    }
                }
            }
            if let Some(b) = &genj_base {
                let j = twime_model::pn::genj::open(b, &a).unwrap_or_else(|e| fail(&format!("genj: {e}")));
                a.set_genj(j);
            }
            if let Some(f) = &wordbonus_file {
                let w = twime_model::wordbonus::WordBonus::open(&a, f).unwrap_or_else(|e| fail(&format!("wordbonus {}: {e}", f.display())));
                a.set_wordbonus(Some(Arc::new(w)));
            }
            a
        }
        Err(e) => {
            eprintln!("twime-engined: cannot load assets: {e}");
            std::process::exit(2);
        }
    };
    // An install replaces pn.cfg (deleted first, written last) around the lexicon files: pn.cfg appearing, vanishing or
    // changing while we loaded means a mix of two versions (or PN off by mistake); exit, the shell starts a fresh engine.
    if let Some((pc, mt)) = &pn_file
        && mtime(pc) != *mt
    {
        fail(&format!("{} changed while loading", pc.display()));
    }
    if let Some((cc, mt)) = &cand_file
        && mtime(cc) != *mt
    {
        fail(&format!("{} changed while loading", cc.display()));
    }
    // CACHE plan 2.2: the cache works on the PN path only.
    let cache = if assets.pn().is_some() { cache_cfg } else { None };
    // Effective decoding settings (CAND-plan 5.5: the switch tool compares them; file contents are checked by hash).
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    user_data |= user_dir.is_some() || exe_dir.as_ref().is_some_and(|d| d.join("user-data.on").is_file());
    let legacy_toggle = exe_dir.as_deref().map(twime_userdata::legacy_defaults).unwrap_or_default().toggle_key;
    let eff = format!(
        "model={} lambda={} norm={} pn={} cand={} cache={} int8={} user={}",
        if no_model { "none".to_string() } else { "loaded".to_string() },
        lambda,
        if norm { "all" } else { "off" },
        assets.pn().map_or("off".to_string(), |(l, c)| format!("on:{}:{:?}:{:?}:{:?}:{}", l.len(), c.b2, c.b3, c.h, c.cands)),
        assets.cand().map_or("off".to_string(), |c| format!("on:{}:{}", c.lex.len(), c.m)),
        cache.as_ref().map_or("off".to_string(), twime_model::cache::CacheCfg::describe),
        if int8 { "on" } else { "off" },
        if user_data { "on" } else { "off" },
    );
    // USERLEX: the ready line is unchanged; the rules are reported on their own line before it.
    let userlex_line = format!("userlex force={} bonus2={}", if user_force { "on" } else { "off" }, user_bonus2);
    opts.doc_text = cache.is_some();
    // FUZZY F1 / F2 share one confusion-class index (built only when either is on)
    let fuzzy_index = repair_data.map(|data| Arc::new(twime_core::fuzzy::Index::from_data_txt(&assets, &data)));
    let repair = repair_mode.zip(fuzzy_index.clone()).map(|(mode, index)| twime_core::Repair { mode, index });
    let fuzzy_words = fuzzy_index.filter(|_| fuzzy_on);
    // AUTOCOMP-LIST: the prefix index is built before ready, not on the first opened list
    if autocomp_list_on {
        assets.warm_autocomp();
    }
    if autocomp_pop_on && autocomp_list_on {
        // SETTINGS-AUTOCOMP: an optional feature must not stop typing; a broken completion LM turns the popup off (one warning).
        assets.completion_lm = dir.as_deref().map(twime_model::pop::CompletionLm::load_optional).transpose().unwrap_or_else(|e| { eprintln!("twime-engined: completion LM unusable, popup off: {e}"); None }).flatten();
    }
    // EMOJI: the table is read only when on; a missing emoji.tsv leaves the feature off (one warning), a malformed one stops startup
    let emoji_table = if emoji_on { twime_engined::load_emoji(dir.as_deref()).unwrap_or_else(|e| fail(&e)) } else { None };
    let pop_lm_hashes = assets.completion_lm.as_ref().map(|lm| (lm.weights_sha256.clone(), lm.vocab_sha256.clone()));
    let assets = Arc::new(assets);
    let user = if user_data {
        let service = twime_userdata::Service::open(user_dir.unwrap_or_else(twime_userdata::user_dir), assets.clone(), exe_dir);
        let writer = service.spawn_writer();
        let poller = service.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            poller.poll();
        });
        Some(UserLink { service, writer })
    } else { None };
    let engine = Engine::with_user(assets, Config {
        lexux, cache, lambda, candidates: cands, word_candidates: words, beta, default_english, punct_half, symbol_table, repair, fuzzy_hints: fuzzy_hints_on && fuzzy_words.is_some() && lexux.is_some(),
        autocomp: (autocomp_on && lexux.is_some()).then_some(twime_model::autocomp::AutoParams::DEPLOYED), autocomp_list: autocomp_list_on, autocomp_pop: autocomp_pop_on, fuzzy_words, emoji: emoji_table.clone(), ..Config::default() }, user, legacy_toggle);
    if let Some(path) = measure_touch {
        let input = std::fs::read_to_string(path).unwrap_or_else(|_| fail("measurement input"));
        let mut session = engine.new_session();
        session.set_ux(true);
        session.set_lexux_peer(true);
        let mut count = 0;
        for syllable in input.split_whitespace().cycle() {
            for c in syllable.chars() {
                let key = twime_core::keymap::key_of(c).unwrap_or_else(|| fail("measurement key"));
                session.on_key(twime_proto::KeyEvent::press(key));
                count += 1;
            }
            if !syllable.ends_with(['ˊ','ˇ','ˋ','˙']) {
                session.on_key(twime_proto::KeyEvent::press(twime_proto::Key::Space)); count += 1;
            }
            if count >= 100 { break }
        }
        if count < 100 { fail("measurement warmup count") }
        session.on_key(twime_proto::KeyEvent::press(twime_proto::Key::Esc));
        let (pages, bytes, checksum) = engine.assets().genw().map_or((0,0,0), |l| l.measure_touch_pages());
        println!("measure_touch warmup_keys={count} pages={pages} bytes={bytes} checksum={checksum}");
    }

    // The pipe is created after loading: until then a shell finds no pipe and passes keys through.
    let server = match PipeServer::with_name(pipe) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("twime-engined: cannot create the pipe (another engine running?): {e}");
            std::process::exit(1);
        }
    };
    // INT8: checked after the pipe exists, so a model.cfg published later is seen by the installer's stop_engine.
    if let Some((mc, mt)) = &model_file
        && mtime(mc) != *mt
    {
        fail(&format!("{} changed while loading", mc.display()));
    }
    // Readers may close stdout after ready. Write the startup announcement together;
    // a closed diagnostic stream must not terminate the pipe service.
    let mut startup = Vec::new();
    writeln!(startup, "ready pid={} load_ms={:.0} pipe={} eff={}", std::process::id(), t.elapsed().as_secs_f64() * 1000.0, server.name(), eff).unwrap();
    writeln!(startup, "genj {}", engine.assets().genj().map_or_else(|| "off".into(), |(l,c)| format!("on count={} {}", l.len(), c.describe()))).unwrap();
    if let Some(w) = engine.assets().wordbonus() {
        writeln!(startup, "wordbonus on rows={}", w.len()).unwrap();
    }
    match lexux {
        Some(c) => writeln!(startup, "lexux lexux_a={} lexux_b={} lexux_w={}", c.a, c.b.map_or_else(|| "off".into(), |b| b.to_string()), if c.w { "on" } else { "off" }).unwrap(),
        None => writeln!(startup, "lexux off").unwrap(),
    }
    // After the ready line (tools read the first line), and only when the rules differ from the old behaviour.
    if user_force || user_bonus2 != twime_model::user::DEFAULT_BONUS {
        writeln!(startup, "{userlex_line}").unwrap();
    }
    // SHORT-PRIOR: only when a key is set (the ready line and the off behaviour are unchanged).
    if !short.describe().is_empty() {
        writeln!(startup, "shortprior {}{}", short.describe(), short_table.as_ref().map_or(String::new(), |t| format!(" word_short_table={}", t.display()))).unwrap();
    }
    // FUZZY: only when the repair is on (the ready line and the off behaviour are unchanged).
    if let Some(m) = repair_mode {
        writeln!(startup, "repair mode={}", if m == twime_core::RepairMode::Auto { "auto" } else { "suggest" }).unwrap();
    }
    // FUZZY F2: only when on (the ready line and the off behaviour are unchanged).
    if fuzzy_on {
        writeln!(startup, "fuzzy words=on").unwrap();
    }
    // FUZZY F2 step 2: only when it is in effect (needs fuzzy_words and LEXUX).
    // AUTOCOMP-LIST: only when on (the ready line and the off behaviour are unchanged).
    if autocomp_pop_on {
        if let Some((weights, vocab)) = pop_lm_hashes {
            writeln!(startup, "autocomp_pop=on lm=int8-S32 weights_sha256={weights} vocab_sha256={vocab}").unwrap();
        } else { writeln!(startup, "autocomp_pop=on effective=off").unwrap(); }
    }
    if autocomp_list_on {
        writeln!(startup, "autocomp_list=on").unwrap();
    }
    // SETTINGS-AUTOCOMP: only when the user-level file exists (absent keeps the ready line unchanged).
    if ac_loaded != twime_userdata::autocomp_cfg::Loaded::Absent {
        writeln!(startup, "{}", twime_engined::autocomp_user_summary(ac_loaded)).unwrap();
    }
    // EMOJI: only when in effect (the table loaded).
    if let Some(t) = &emoji_table {
        writeln!(startup, "emoji=on keys={}", t.len()).unwrap();
    }
    // AUTOCOMP: only when it is in effect (needs LEXUX).
    if autocomp_on && lexux.is_some() {
        writeln!(startup, "autocomp=on").unwrap();
    }
    if fuzzy_hints_on && fuzzy_on && lexux.is_some() {
        writeln!(startup, "fuzzy hints=on").unwrap();
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&startup);
    let _ = stdout.flush();
    drop(stdout);
    // Explicit measurement processes finish themselves after their runner has sampled memory.
    // Normal engine startup creates no watcher and retains the previous lifetime.
    if let Some(done) = measure_done {
        std::thread::spawn(move || loop {
            if done.is_file() { std::process::exit(0); }
            std::thread::sleep(std::time::Duration::from_millis(50));
        });
    }
    if let Err(e) = serve(engine, server, opts, lock) {
        eprintln!("twime-engined: {e}");
        std::process::exit(1);
    }
}

fn uninstall_after_lock(lock: &RunLock, exe: Option<PathBuf>) -> bool {
    if exe.and_then(|p| p.parent().map(|d| d.join(twime_proto::ids::UNINSTALL_MARKER))).is_some_and(|m| m.exists()) {
        lock.release(false);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_appearing_after_initial_check_releases_acquired_lock() {
        let dir = std::env::temp_dir().join(format!("twime-startup-test-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("twime-engined.exe");
        let marker = dir.join(twime_proto::ids::UNINSTALL_MARKER);
        assert!(!marker.exists()); // The engine passes its initial check.
        // Installer writes its marker and observes no pipe or running.lock before the engine takes the lock.
        std::fs::write(&marker, b"1\n").unwrap();
        assert!(!dir.join("running.lock").exists());
        let lock = RunLock::acquire(&dir, "test-logon".into()).unwrap();
        assert!(uninstall_after_lock(&lock, Some(exe.clone())));
        #[cfg(windows)]
        assert!(!dir.join("running.lock").exists());
        #[cfg(not(windows))] // kept and emptied (see runlock.rs)
        assert_eq!(std::fs::read(dir.join("running.lock")).unwrap(), b"");
        assert!(marker.exists());
        std::fs::remove_file(&marker).unwrap();
        let lock = RunLock::acquire(&dir, "test-logon".into()).unwrap();
        assert!(!uninstall_after_lock(&lock, Some(exe)));
        assert!(dir.join("running.lock").exists());
        lock.release(false);
        assert!(dir.is_absolute() && dir.starts_with(std::env::temp_dir()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("twime-engined: {msg}");
    std::process::exit(2);
}

fn usage(msg: &str) -> ! {
    eprintln!("twime-engined: {msg}\nsee the comment at the top of src/main.rs");
    std::process::exit(2);
}

// No dev-mode exception: the engine must not serve with high or service-account privileges.
#[cfg(windows)]
fn engine_token_refusal(rid: u32, sid: &str) -> Option<&'static str> {
    if matches!(sid, "S-1-5-18" | "S-1-5-19" | "S-1-5-20") {
        Some("service-account token is not allowed")
    } else if rid >= 0x3000 {
        Some("high or higher integrity token is not allowed")
    } else {
        None
    }
}

#[cfg(all(test, windows))]
mod token_policy_tests {
    use super::engine_token_refusal;
    #[test]
    fn rejects_high_and_service_accounts_without_dev_exception() {
        for rid in [0, 0x1000, 0x2000, 0x2100, 0x2fff] {
            assert_eq!(engine_token_refusal(rid, "S-1-5-21-123"), None);
            for sid in ["S-1-5-18", "S-1-5-19", "S-1-5-20"] {
                assert!(engine_token_refusal(rid, sid).is_some());
            }
        }
        for rid in [0x3000, 0x4000, u32::MAX] {
            assert!(engine_token_refusal(rid, "S-1-5-21-123").is_some());
        }
    }
}
