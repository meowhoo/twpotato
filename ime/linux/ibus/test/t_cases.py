"""Assertions 1 and 2 of LINUX-plan 7.2, the candidate page of assertion 3, and the latency threshold, over
the input set, on IBus: every case is typed through ibus-daemon and compared, reply by reply, with what the engine
itself answers to the same keys. The same measure as fcitx5/test/t_cases.py: the engine-level reference (ime/xcmp,
protocol 5, every reply), its cache and its reading are that module's.

Per case, as xcmp types it: focus, the keys of every syllable, Down (the list opens), Esc, Enter.
Compared at every key: handled, the text committed by that key, the preedit text and its caret (characters on both
sides). At Down: the candidates of the page shown, from the lookup table the panel (this program) receives.
At the end: everything committed."""
import importlib.util, os, time

from ib import *
from zhuyin import keys_of_line

_spec = importlib.util.spec_from_file_location("fcitx5_t_cases", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "fcitx5", "test", "t_cases.py"))
ref = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ref)
OUT = os.environ["TWFX_OUT"]


def run():
    lines = ref.load_cases()
    assert lines, "the input set is empty"
    tag = str(len(lines))
    out_path, steps_path = ref.reference(lines, tag)
    ref_out = [l.split("\t") for l in open(out_path, encoding="utf-8").read().split("\n") if l]
    ref_steps = ref.read_steps(steps_path)
    assert len(ref_out) == len(ref_steps) == len(lines)

    d = Ibus(os.path.join(STAGE, "ibus.log"))
    ok = False
    try:
        ic, panel = d.ib.context(), d.ib.panel
        ic.focus_in()
        ic.set_engine(ENGINE)
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
        (shell_pid,) = stage_procs(SHELL)
        ic.focus_out()

        lat = {"convert": [], "slot": [], "list": [], "other": [], "commit": []}
        bad = []  # (case id, reply number, kind, what, expected, got)
        bad_cases = set()
        n_steps = 0
        started = time.monotonic()
        for n, (line, out, steps) in enumerate(zip(lines, ref_out, ref_steps)):
            cid, _set, _expected, zhuyin = line.split("\t")
            assert out[0] == cid
            syms = [ord(k) for s in keys_of_line(zhuyin) for k in s] + [DOWN, ESC, RETURN]
            assert len(steps) == len(syms) + 1 and steps[0][0] == "focus", f"case {cid}: {len(steps)} replies for {len(syms)} keys"
            ic.ic.focus_in()  # no waiting: the daemon handles it before the key that follows
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
                if h != handled:
                    differ(i, kind, "handled", handled, h)
                if "".join(ic.commits[before:]) != commit:
                    differ(i, kind, "commit", commit, ic.commits[before:])
                if ic.preedit != state:
                    differ(i, kind, "preedit", state, ic.preedit)
                cands = panel.cands
                if kind == "list":
                    got = "/".join(t for _, t in cands["items"]) if cands else ""
                    if got != out[3]:
                        differ(i, kind, "candidates", out[3], got)
                    if cands and (cands["index"] != 0 or cands["page_size"] != len(cands["items"])
                                  or [l for l, _ in cands["items"]] != [str(j) for j in range(1, len(cands["items"]) + 1)]):
                        differ(i, kind, "labels / highlight / page", "1..n, 0, n", cands)
                elif cands is not None:
                    differ(i, kind, "candidates", "closed", cands)
                ic.key(sym, release=True)
            if "".join(ic.commits) != out[4]:
                differ(len(syms), "case", "all commits", out[4], "".join(ic.commits))
            if ic.preedit[0]:  # only after a difference: leave nothing behind for the next case
                ic.reset()
            ic.ic.focus_out()
            if (n + 1) % 2000 == 0:
                print(f"  {n + 1} cases, {len(bad_cases)} differ, {time.monotonic() - started:.0f} s", flush=True)
        took = time.monotonic() - started
        still = engines() == [engine_pid] and stage_procs(SHELL) == [shell_pid]
        peak = 0
        for l in open(f"/proc/{engine_pid}/status") if still else []:
            if l.startswith("VmHWM:"):
                peak = int(l.split()[1]) / 1024
        ic.destroy()

        conv = sorted(lat["convert"])
        every = sorted(x for v in lat.values() for x in v)
        over = sum(x > 0.010 for x in conv)
        pct = ref.pct
        report = [
            f"cases: {len(lines)}; replies compared: {n_steps}; cases with a difference: {len(bad_cases)}; time {took:.0f} s",
            f"engine and shell: the same processes for the whole run: {still}; engine peak resident {peak:.1f} MB",
            f"cold start: input possible {cold:.2f} s after the first key",
            "process_key_event round trip, conversion keys (the last key of every syllable): "
            f"n={len(conv)} p50={pct(conv, 50) * 1000:.2f} ms p95={pct(conv, 95) * 1000:.2f} ms p99={pct(conv, 99) * 1000:.2f} ms "
            f"max={(conv[-1] if conv else 0) * 1000:.2f} ms over10ms={over}",
            "process_key_event round trip, every key press: "
            f"n={len(every)} p50={pct(every, 50) * 1000:.2f} ms p95={pct(every, 95) * 1000:.2f} ms p99={pct(every, 99) * 1000:.2f} ms "
            f"max={(every[-1] if every else 0) * 1000:.2f} ms over10ms={sum(x > 0.010 for x in every)}",
        ]
        for cid, i, kind, what, want, got in bad:
            report.append(f"DIFF {cid} reply {i} ({kind}) {what}: engine {want!r} ibus {got!r}")
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
        code = d.stop()
    return ok and code in (0, -15)
