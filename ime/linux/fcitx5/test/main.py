"""Runs the parts named on the command line (func, cases, gtk) inside the environment run.sh prepared.
Exit code 0: every part that ran passed."""
import inspect, os, sys, time, traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fx


def run_func(only=None):
    import t_func
    tests = [(n, f) for n, f in vars(t_func).items() if n.startswith("t") and n[1:2] in "_34567" and inspect.isfunction(f)]
    if only:
        tests = [(n, f) for n, f in tests if only in n]
    if not tests:
        print(f"func: no test matches {only!r}")
        return False
    failed = []
    for name, test in tests:
        if os.path.exists(t_func.STARTS):
            os.unlink(t_func.STARTS)
        fx.write_fcitx_config(stock_hotkeys=getattr(test, "stock_hotkeys", False))
        kim = fx.Kim() if getattr(test, "kim", False) else None
        print(f"{name} ...", flush=True)
        t = time.monotonic()
        f = fx.Fcitx(t_func.LOG, enable=("kimpanel",) if kim else ())
        try:
            test(f, kim) if kim else test(f)
            ok = True
        except Exception:
            traceback.print_exc()
            ok = False
        try:
            code = f.stop()
            if code != 0:
                print(f"    fcitx5 ended with code {code}")
                ok = False
        except Exception:
            traceback.print_exc()
            ok = False
        if kim:
            kim.close()
        if fx.stage_procs("fcitx5") or fx.engines():
            print("    processes left after the test")
            ok = False
        print(f"{name}: {'ok' if ok else 'FAILED'} ({time.monotonic() - t:.1f} s)", flush=True)
        if not ok:
            failed.append(name)
    fx.write_fcitx_config()
    print(f"func: {len(tests) - len(failed)} of {len(tests)} passed" + (f"; FAILED: {', '.join(failed)}" if failed else ""))
    return not failed


def main():
    parts = sys.argv[1:]
    results = {}
    for part in parts:
        name, _, arg = part.partition(":")
        if name == "func":
            results[part] = run_func(arg or None)
        elif name == "cases":
            import t_cases
            results[part] = t_cases.run()
        elif name == "gtk":
            import t_gtk
            results[part] = t_gtk.run()
        else:
            sys.exit(f"unknown part {part!r}")
    print("---- summary ----")
    for part, ok in results.items():
        print(f"{part}: {'passed' if ok else 'FAILED'}")
    sys.exit(0 if all(results.values()) else 1)


main()
