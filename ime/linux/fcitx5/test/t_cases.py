"""Assertions 1 and 2 of LINUX-plan 7.2, the candidate page of assertion 3, and the latency threshold, over
the input set: every case is typed through fcitx5 and compared, reply by reply, with what the engine itself answers to
the same keys.

The engine-level reference comes from ime/xcmp, run here with protocol version 5 and XCMP_STEPS=1 (every reply on
standard error), with user data on (an empty user lexicon) as the shell starts the engine. It is cached in TWFX_OUT and
made again when the engine, the assets' model.cfg or the input set is newer.

Per case, as xcmp types it: focus, the keys of every syllable, Down (the list opens), Esc, Enter.
Compared at every key: handled, the text committed by that key, the preedit text and its caret (the engine counts
characters, Fcitx5 UTF-8 bytes). At Down: the candidates of the page shown. At the end: everything committed.
"""
import os, re, subprocess, time

from fx import *
from zhuyin import keys_of_line

OUT = os.environ["TWFX_OUT"]
ENGINE, ASSETS, XCMP = os.environ["TWFX_ENGINE"], os.environ["TWFX_ASSETS"], os.environ["TWFX_XCMP"]
STEP = re.compile(r'^(true|false)\|(true|false)\|(true|false)\|(?:None|Some\("((?:[^"\\]|\\.)*)"\))\|(?:None|Some\(\("((?:[^"\\]|\\.)*)", (\d+)\)\))$')


def unescape(s):
    """A string as Rust's {:?} prints it."""
    if s is None or "\\" not in s:
        return s
    s = re.sub(r"\\u\{([0-9a-fA-F]+)\}", lambda m: chr(int(m.group(1), 16)), s)
    return re.sub(r"\\(.)", lambda m: {"n": "\n", "t": "\t", "r": "\r", "0": "\0"}.get(m.group(1), m.group(1)), s)


def load_cases():
    limit = int(os.environ.get("CASES_LIMIT") or 0)
    lines = [l for l in open(os.environ["TWFX_CASES"], encoding="utf-8").read().split("\n") if l]
    return lines[:limit] if limit else lines


def reference(lines, tag, user_data=True):
    """Runs xcmp over these cases unless a fresh result is cached. Returns (path of out.tsv, path of the replies)."""
    prefix = os.path.join(OUT, f"ref-{tag}-v5{'ul' if user_data else ''}")
    out, steps, cases = prefix + ".out.tsv", prefix + ".steps", prefix + ".cases.tsv"
    text = "\n".join(lines) + "\n"
    newest = max(os.path.getmtime(p) for p in (ENGINE, XCMP, os.path.join(ASSETS, "model.bin")))
    if (os.path.exists(out) and os.path.exists(steps) and os.path.exists(cases) and open(cases, encoding="utf-8").read() == text
            and os.path.getmtime(out) > newest):
        return out, steps
    open(cases, "w", encoding="utf-8").write(text)
    scratch = os.path.join(STAGE, f"xcmp-{tag}-{int(user_data)}")
    args = [XCMP, ENGINE, ASSETS, cases, prefix, scratch, "5"]
    if user_data:
        user = os.path.join(STAGE, f"xcmp-user-{tag}")
        os.makedirs(user)
        open(os.path.join(user, "userlex.tsv"), "w").close()
        args.append(user)
    print(f"reference: xcmp over {len(lines)} cases (protocol 5, user data {'on' if user_data else 'off'}) ...", flush=True)
    t = time.monotonic()
    with open(steps + ".tmp", "wb") as err:
        subprocess.run(args, check=True, env=dict(os.environ, XCMP_STEPS="1"), stdout=subprocess.DEVNULL, stderr=err)
    os.replace(steps + ".tmp", steps)
    print(f"reference: done in {time.monotonic() - t:.0f} s", flush=True)
    return out, steps


def read_steps(path):
    """The replies of every case in order: lists of (kind, handled, commit, composing, composition or None)."""
    cases, cur = [], None
    for line in open(path, encoding="utf-8"):
        _seq, kind, step = line.rstrip("\n").split("\t", 2)
        m = STEP.match(step)
        assert m, f"unreadable reply line: {line!r}"
        if kind == "focus":
            cur = []
            cases.append(cur)
        comp = None if m.group(5) is None else (unescape(m.group(5)), int(m.group(6)))
        cur.append((kind, m.group(1) == "true", unescape(m.group(4)) or "", m.group(2) == "true", comp))
    return cases


def pct(sorted_values, p):
    return sorted_values[max(1, min(len(sorted_values), -(-len(sorted_values) * p // 100))) - 1] if sorted_values else 0


def run():
    lines = load_cases()
    assert lines, "the input set is empty"
    tag = str(len(lines))
    out_path, steps_path = reference(lines, tag)
    plain_path, _ = reference(lines, tag, user_data=False)
    ref_out = [l.split("\t") for l in open(out_path, encoding="utf-8").read().split("\n") if l]
    plain = open(plain_path, encoding="utf-8").read().split("\n")
    with_user = open(out_path, encoding="utf-8").read().split("\n")
    differing = sum(a != b for a, b in zip(plain, with_user))
    print(f"reference: user data on / off differ in {differing} of {len(ref_out)} cases (xcmp out.tsv lines)")
    ref_steps = read_steps(steps_path)
    assert len(ref_out) == len(ref_steps) == len(lines)

    write_fcitx_config()
    f = Fcitx(os.path.join(STAGE, "fcitx.log"))
    ok = False
    try:
        ic = f.fx.context()
        ic.focus_in()
        t = time.monotonic()
        ic.tap(ord("s"))
        while True:
            ic.tap(CTRL_L)
            try:
                if engine_status()[0] >= 1:
                    break
            except OSError:
                pass
            assert time.monotonic() - t < 10, "no engine"
            time.sleep(0.01)
        cold = time.monotonic() - t
        (engine_pid,) = engines()
        ic.focus_out()

        lat = {"convert": [], "slot": [], "list": [], "other": [], "commit": []}
        bad = []  # (case id, reply number, kind, what, expected, got)
        bad_cases = set()
        n_steps = 0
        started = time.monotonic()
        for n, (line, ref, steps) in enumerate(zip(lines, ref_out, ref_steps)):
            cid, _set, _expected, zhuyin = line.split("\t")
            assert ref[0] == cid
            syllables = keys_of_line(zhuyin)
            syms = [ord(k) for s in syllables for k in s] + [DOWN, ESC, RETURN]
            assert len(steps) == len(syms) + 1 and steps[0][0] == "focus", f"case {cid}: {len(steps)} replies for {len(syms)} keys"
            ic.focus_in()
            ic.events.clear()
            ic.commits.clear()
            state = ("", 0)  # expected preedit: text, caret in characters

            def differ(i, kind, what, want, got):
                bad_cases.add(cid)
                if len(bad) < 200:
                    bad.append((cid, i, kind, what, want, got))

            for i, sym in enumerate(syms, 1):
                kind, handled, commit, composing, comp = steps[i]
                if comp is not None:
                    state = comp
                elif not composing:
                    state = ("", 0)
                before = len(ic.commits)
                h, dt = ic.key(sym)
                lat[kind].append(dt)
                n_steps += 1
                want = (state[0], len(state[0][:state[1]].encode("utf-8")))
                if h != handled:
                    differ(i, kind, "handled", handled, h)
                if "".join(ic.commits[before:]) != commit:
                    differ(i, kind, "commit", commit, ic.commits[before:])
                if ic.preedit != want:
                    differ(i, kind, "preedit", want, ic.preedit)
                if kind == "list":
                    got = "/".join(t for _, t in ic.cands["items"]) if ic.cands else ""
                    if got != ref[3]:
                        differ(i, kind, "candidates", ref[3], got)
                    if ic.cands and (ic.cands["index"] != 0 or [l for l, _ in ic.cands["items"]] != [f"{j}. " for j in range(1, len(ic.cands["items"]) + 1)]):
                        differ(i, kind, "labels / highlight", "1..n, 0", ic.cands)
                elif ic.cands is not None:
                    differ(i, kind, "candidates", "closed", ic.cands)
                ic.key(sym, release=True)
            if "".join(ic.commits) != ref[4]:
                differ(len(syms), "case", "all commits", ref[4], "".join(ic.commits))
            if ic.preedit[0]:  # only after a difference: leave nothing behind for the next case
                ic.reset()
            ic.focus_out()
            if (n + 1) % 2000 == 0:
                print(f"  {n + 1} cases, {len(bad_cases)} differ, {time.monotonic() - started:.0f} s", flush=True)
        took = time.monotonic() - started
        still = engines() == [engine_pid]
        peak = 0
        for l in open(f"/proc/{engine_pid}/status") if still else []:
            if l.startswith("VmHWM:"):
                peak = int(l.split()[1]) / 1024
        ic.destroy()

        conv = sorted(lat["convert"])
        every = sorted(x for v in lat.values() for x in v)
        over = sum(x > 0.010 for x in conv)
        report = [
            f"cases: {len(lines)}; replies compared: {n_steps}; cases with a difference: {len(bad_cases)}; time {took:.0f} s",
            f"engine: the same process for the whole run: {still}; peak resident {peak:.1f} MB",
            f"cold start: input possible {cold:.2f} s after the first key",
            "ProcessKeyEvent round trip, conversion keys (the last key of every syllable): "
            f"n={len(conv)} p50={pct(conv, 50) * 1000:.2f} ms p95={pct(conv, 95) * 1000:.2f} ms p99={pct(conv, 99) * 1000:.2f} ms "
            f"max={(conv[-1] if conv else 0) * 1000:.2f} ms over10ms={over}",
            "ProcessKeyEvent round trip, every key press: "
            f"n={len(every)} p50={pct(every, 50) * 1000:.2f} ms p95={pct(every, 95) * 1000:.2f} ms p99={pct(every, 99) * 1000:.2f} ms "
            f"max={(every[-1] if every else 0) * 1000:.2f} ms over10ms={sum(x > 0.010 for x in every)}",
        ]
        for cid, i, kind, what, want, got in bad:
            report.append(f"DIFF {cid} reply {i} ({kind}) {what}: engine {want!r} fcitx5 {got!r}")
        p99_ok = bool(conv) and pct(conv, 99) <= 0.010  # no sample is not a pass
        ok = not bad_cases and p99_ok and still
        report.append(f"assertions 1, 2 and the candidate page: {'passed' if not bad_cases else 'FAILED'}; latency p99 <= 10 ms: {'passed' if p99_ok else 'FAILED'}")
        text = "\n".join(report) + "\n"
        open(os.path.join(OUT, f"cases-{tag}.result.txt"), "w", encoding="utf-8").write(text)
        with open(os.path.join(OUT, f"cases-{tag}.lat.tsv"), "w") as lf:
            for kind, values in lat.items():
                for x in values:
                    lf.write(f"{kind}\t{x * 1e6:.0f}\n")
        print(text, end="")
    finally:
        code = f.stop()
    return ok and code == 0
