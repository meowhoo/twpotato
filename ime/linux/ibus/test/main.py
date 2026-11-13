"""Runs the parts named on the command line (component, func, cases, gtk, both) inside the environment run.sh prepared.
Exit code 0: every part that ran passed.

A process can talk to one ibus-daemon only (ib.py), so every functional test and every other part runs in a process
of its own: `main.py one NAME` and `main.py part NAME` are those processes."""
import inspect, os, subprocess, sys, time, traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import ib


def run_one(name):
    """One functional test with its own ibus-daemon. True: passed and nothing was left running."""
    import t_func
    test = getattr(t_func, name)
    if os.path.exists(t_func.STARTS):
        os.unlink(t_func.STARTS)
    d = ib.Ibus(t_func.LOG)
    ok = True
    try:
        test(d)
    except Exception:
        traceback.print_exc()
        ok = False
    try:
        code = d.stop()
        if code not in (0, -15):  # ibus-daemon has no handler for SIGTERM: ended by the signal is its normal end
            print(f"    ibus-daemon ended with code {code}")
            ok = False
    except Exception:
        traceback.print_exc()
        ok = False
    if ib.stage_procs("ibus-daemon") or ib.stage_procs(ib.SHELL) or ib.engines():
        print("    processes left after the test")
        ok = False
    return ok


def run_func(only=None):
    import t_func
    tests = [n for n, f in vars(t_func).items() if n.startswith("t") and n[1:2] in "_345678" and inspect.isfunction(f)]
    if only:
        tests = [n for n in tests if only in n]
    if not tests:
        print(f"func: no test matches {only!r}")
        return False
    failed = []
    for name in tests:
        print(f"{name} ...", flush=True)
        t = time.monotonic()
        ok = subprocess.run([sys.executable, os.path.abspath(__file__), "one", name]).returncode == 0
        print(f"{name}: {'ok' if ok else 'FAILED'} ({time.monotonic() - t:.1f} s)", flush=True)
        if not ok:
            failed.append(name)
    print(f"func: {len(tests) - len(failed)} of {len(tests)} passed" + (f"; FAILED: {', '.join(failed)}" if failed else ""))
    return not failed


def main():
    parts = sys.argv[1:]
    if parts[:1] == ["one"]:
        sys.stdout.reconfigure(line_buffering=True)
        sys.exit(0 if run_one(parts[1]) else 1)
    if parts[:1] == ["part"]:
        sys.stdout.reconfigure(line_buffering=True)
        sys.exit(0 if __import__("t_" + parts[1]).run() else 1)
    results = {}
    for part in parts:
        name, _, arg = part.partition(":")
        if name == "func":
            results[part] = run_func(arg or None)
        elif name in ("component", "cases", "gtk", "both"):
            results[part] = subprocess.run([sys.executable, os.path.abspath(__file__), "part", name]).returncode == 0
        else:
            sys.exit(f"unknown part {part!r}")
    print("---- summary ----")
    for part, ok in results.items():
        print(f"{part}: {'passed' if ok else 'FAILED'}")
    sys.exit(0 if all(results.values()) else 1)


main()
