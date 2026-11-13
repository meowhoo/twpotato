"""One framework's part of check-installed.sh (which prepared the environment): types through the installed TwPotato
and checks which files served. `check-installed.py ibus|fcitx5`; exit code 0: passed."""
import glob, os, signal, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
TESTS = {"ibus": os.path.join(HERE, "..", "ibus", "test"), "fcitx5": os.path.join(HERE, "..", "fcitx5", "test")}
ENGINE_EXE, ASSETS = "/usr/lib/twpotato/twime-engined", "/usr/share/twpotato/assets"
SHELL_EXE = "/usr/lib/twpotato/ibus-engine-twpotato"
STAGE = os.environ["TWFX_STAGE"]


def exe(pid):
    return os.readlink(f"/proc/{pid}/exe")


def check_engine(fx):
    pids = fx.engines()
    assert len(pids) == 1, f"engines: {pids}"
    assert exe(pids[0]) == ENGINE_EXE, exe(pids[0])
    args = open(f"/proc/{pids[0]}/cmdline", "rb").read().split(b"\0")
    assert args[args.index(b"--assets-dir") + 1] == ASSETS.encode(), args
    print(f"  engine: {ENGINE_EXE} --assets-dir {ASSETS}")


def type_hello(ic, typ, return_key):
    typ(ic, "su3cl3")
    assert ic.preedit[0] == "你好", ic.preedit
    ic.tap(return_key)
    assert ic.commits == ["你好"], ic.commits
    print("  su3cl3 + Enter: one commit of 你好")


def ibus():
    import ib, t_func
    assert os.path.isfile("/usr/share/ibus/component/twpotato.xml"), "no component file"
    # ibus-daemon reads the system's component directory (no IBUS_COMPONENT_PATH): every installed engine, not only
    # ours. The test client's connection to a daemon that had just started was seen to fail once (ib.Ib:
    # "bus.is_connected()", right after a package upgrade; not seen again in 12 more starts), before anything of ours
    # is involved; such a start is tried again.
    for attempt in range(3):
        try:
            d = ib.Ibus(os.path.join(STAGE, "ibus.log"))
            break
        except AssertionError:
            if attempt == 2:
                raise
            print("  (the test client did not get its connection to ibus-daemon: starting the daemon again)")
            for pid in ib.stage_procs("ibus-daemon"):
                os.kill(pid, signal.SIGKILL)
            while ib.stage_procs("ibus-daemon"):
                time.sleep(0.05)
    try:
        ic = d.ib.context()
        ic.focus_in()
        ic.set_engine(ib.ENGINE)
        t_func.ready(ic)
        type_hello(ic, t_func.typ, ib.RETURN)
        shells = ib.stage_procs(ib.SHELL)
        assert len(shells) == 1 and exe(shells[0]) == SHELL_EXE, [exe(p) for p in shells]
        print(f"  front end: {SHELL_EXE}")
        check_engine(ib)
    finally:
        d.stop()


def fcitx5():
    import fx, t_func
    addons = glob.glob("/usr/lib/*/fcitx5/twpotato.so")
    assert len(addons) == 1, addons
    fx.write_fcitx_config()
    f = fx.Fcitx(os.path.join(STAGE, "fcitx.log"))
    try:
        ic = t_func.context(f)
        type_hello(ic, t_func.typ, fx.RETURN)
        maps = open(f"/proc/{f.p.pid}/maps").read()
        assert addons[0] in maps, "fcitx5 has not loaded the installed addon"
        print(f"  front end: {addons[0]} (loaded by fcitx5)")
        check_engine(fx)
    finally:
        f.stop()


if __name__ == "__main__":
    sys.stdout.reconfigure(line_buffering=True)
    sys.path.insert(0, TESTS[sys.argv[1]])  # the helpers of that framework (ib.py adds the shared ones of fcitx5/test)
    {"ibus": ibus, "fcitx5": fcitx5}[sys.argv[1]]()
