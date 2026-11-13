"""Assertion 8 of LINUX-plan 7.2: fcitx5 and ibus-daemon run in the same private session, each with our
shell, and a round of the input set is typed through both in turns. There must be one engine process only, and what
both commit must be what the engine itself gives (the same reference as the cases parts).

The two shells find no engine when their contexts get the focus and each starts one: only one of the two may stay."""
import importlib.util, os, time

from ib import *
import fx
from zhuyin import keys_of_line

_spec = importlib.util.spec_from_file_location("fcitx5_t_cases", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "fcitx5", "test", "t_cases.py"))
ref = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ref)
N = 200
STARTS = os.path.join(STAGE, "engine-starts")


def run():
    lines = ref.load_cases()[:N]
    assert lines, "the input set is empty"
    out_path, _ = ref.reference(lines, str(len(lines)))
    expected = [l.split("\t")[4] for l in open(out_path, encoding="utf-8").read().split("\n") if l]
    assert len(expected) == len(lines)
    if os.path.exists(STARTS):
        os.unlink(STARTS)
    fx.write_fcitx_config()
    d, f, ok = None, None, False
    try:
        d = Ibus(os.path.join(STAGE, "ibus.log"))
        # fcitx5's own IBus frontend takes ibus-daemon's place for IBus clients (it is on by default): turned off here,
        # or there would be no ibus-daemon to test next to fcitx5.
        f = fx.Fcitx(os.path.join(STAGE, "fcitx.log"), disable=("ibusfrontend",))
        a = f.fx.context()
        a.focus_in()
        b = d.ib.context()
        b.focus_in()
        b.set_engine(ENGINE)
        # Each focus starts the engine, one right after the other: the second start usually finds none either and
        # starts another (then only one of the two may stay), but it may also come late enough to find the first one
        # serving: both are right.
        t = time.monotonic()
        while True:
            a.tap(fx.CTRL_L); b.tap(CTRL_L)
            try:
                if engine_status()[0] >= 2 and len(engines()) == 1:
                    break
            except OSError:
                pass
            assert time.monotonic() - t < 15, f"the two shells did not both get a session on one engine: engines {engines()}"
            time.sleep(0.02)
        # Whatever the two first keys left behind is removed before the round.
        a.tap(fx.ESC); b.tap(ESC)
        assert a.preedit[0] == "" and b.preedit[0] == "" and a.commits == [] and b.commits == [], (a.preedit, b.preedit, a.commits, b.commits)
        started = len(open(STARTS).read().split())
        assert started in (1, 2), started
        (engine_pid,) = engines()
        print(f"both: {started} engine start(s) by the two shells, one engine process left (pid {engine_pid}); sessions {engine_status()[0]}")

        bad = []
        for n, (line, want) in enumerate(zip(lines, expected)):
            cid, _set, _expected, zhuyin = line.split("\t")
            syms = [ord(k) for s in keys_of_line(zhuyin) for k in s]
            for name, ic, extra in (("fcitx5", a, (fx.DOWN, fx.ESC, fx.RETURN)), ("ibus", b, (DOWN, ESC, RETURN))):
                ic.focus_out(); ic.focus_in()  # as the reference: every case starts with a new focus
                ic.commits.clear()
                for sym in list(syms) + list(extra):
                    ic.tap(sym)
                if "".join(ic.commits) != want or ic.preedit[0]:
                    bad.append((cid, name, want, "".join(ic.commits), ic.preedit))
            if engines() != [engine_pid]:
                bad.append((cid, "engine", engine_pid, engines(), None))
                break
        for item in bad[:20]:
            print(f"  DIFF {item}")
        sessions = engine_status()
        ok = not bad and engines() == [engine_pid] and sessions[0] == 2 and len(open(STARTS).read().split()) == started
        print(f"both: {len(lines)} cases through each framework in turns, {len(bad)} differ; engine processes: {len(engines())}; "
              f"sessions at the end: {sessions[0]}; engine starts during the round: {len(open(STARTS).read().split()) - started}")
    finally:
        code_f = f.stop() if f else 1
        code_d = d.stop() if d else 1
    return ok and code_f == 0 and code_d in (0, -15)
