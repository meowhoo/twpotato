"""A real toolkit input module in the path (LINUX-plan 7.2, last paragraph): a GTK 3 text field on an Xvfb
display, fcitx5's GTK input module (GTK_IM_MODULE=fcitx), keys sent to the X server with xdotool. Checks only that
every sentence arrives in the field once and whole.

What the field must hold is taken from the D-Bus client first: the same sentences typed the same way (one focus, no
focus change between sentences), which the cases part has compared with the engine."""
import os, subprocess, sys, time

from fx import *
from zhuyin import keys_of_line

HERE = os.path.dirname(os.path.abspath(__file__))
N = 10


def wait_for(what, cond, limit_s=5.0):
    end = time.monotonic() + limit_s
    while not cond():
        assert time.monotonic() < end, f"timeout: {what}"
        time.sleep(0.02)


def run():
    lines = [l.split("\t") for l in open(os.environ["TWFX_CASES"], encoding="utf-8").read().split("\n") if l][:N]
    sentences = ["".join(keys_of_line(l[3])) for l in lines]
    r, w = os.pipe()
    xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(w), "-screen", "0", "800x600x24", "-nolisten", "tcp"], pass_fds=[w],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.close(w)
    display = ":" + os.read(r, 16).decode().strip()
    os.close(r)
    env = dict(os.environ, DISPLAY=display)
    write_fcitx_config()
    f = Fcitx(os.path.join(STAGE, "fcitx.log"), enable=("xcb",), env={"DISPLAY": display})
    box, ok = None, False
    try:
        # What the engine makes of these sentences in one focus (also brings the engine up).
        ic = f.fx.context()
        ic.focus_in()
        t = time.monotonic()
        while True:
            ic.tap(CTRL_L)
            try:
                if engine_status()[0] >= 1:
                    break
            except OSError:
                pass
            assert time.monotonic() - t < 10, "no engine"
            time.sleep(0.02)
        for keys in sentences:
            for k in keys:
                h, _ = ic.tap(ord(k))
                assert h
            ic.tap(RETURN)
        expected = list(ic.commits)
        assert len(expected) == N and all(expected), expected
        ic.focus_out()
        ic.destroy()

        log = os.path.join(STAGE, "gtk-box.log")
        box = subprocess.Popen([sys.executable, os.path.join(HERE, "gtk_box.py"), log],
                               env=dict(env, GTK_IM_MODULE="fcitx", GDK_BACKEND="x11", NO_AT_BRIDGE="1"),
                               stdout=subprocess.DEVNULL, stderr=open(os.path.join(STAGE, "gtk-box.err"), "wb"))

        def entries(kind):
            if not os.path.exists(log):
                return []
            return [l.split("\t", 1)[1] for l in open(log, encoding="utf-8").read().split("\n") if l.startswith(kind + "\t")]

        wait_for("the GTK window", lambda: entries("READY"), 15)
        wid = entries("READY")[0]
        xdo = lambda *a: subprocess.run(["xdotool", *a], env=env, check=True, timeout=30)
        xdo("windowfocus", "--sync", wid)
        time.sleep(0.5)
        for i, keys in enumerate(sentences):
            xdo("type", "--delay", "25", "--", keys)
            wait_for(f"preedit of sentence {i + 1}", lambda: entries("PREEDIT") and entries("PREEDIT")[-1] == expected[i])
            xdo("key", "Return")  # commits the composition
            wait_for(f"commit of sentence {i + 1}", lambda: len(entries("INSERT")) > i)
            time.sleep(0.2)       # anything committed twice would arrive now
            xdo("key", "Return")  # the field reports its text and empties itself
            wait_for(f"line {i + 1}", lambda: len(entries("LINE")) > i)
        time.sleep(0.3)
        got, inserts = entries("LINE"), entries("INSERT")
        for i, (want, have) in enumerate(zip(expected, got)):
            print(f"  {lines[i][0]}: {'ok' if want == have else 'DIFFERENT'} {have}" + ("" if want == have else f" (expected {want})"))
        ok = got == expected and inserts == expected
        if inserts != expected:
            print(f"  texts put into the field: {inserts}")
        print(f"gtk: {sum(a == b for a, b in zip(expected, got))} of {N} sentences arrived once and whole; "
              f"{len(inserts)} insertions for {N} commits")
    finally:
        if box:
            box.terminate()
            box.wait(5)
        code = f.stop()
        xvfb.terminate()
        xvfb.wait(5)
    return ok and code == 0
