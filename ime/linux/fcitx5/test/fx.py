"""Headless Fcitx5 client for the tests: talks to fcitx5's D-Bus frontend (the interface the GTK / Qt input modules use)
on the session bus of the caller. No display is involved.

    f = Fcitx(log); ic = f.fx.context(); ic.focus_in(); handled, seconds = ic.key(keysym); ic.commits, ic.preedit, ic.cands
"""
import os, signal, socket, struct, subprocess, time
from gi.repository import Gio, GLib

SERVICE = "org.fcitx.Fcitx5"
STAGE = os.environ.get("TWFX_STAGE", "")
# fcitx-utils/capabilityflags.h
CAP_PREEDIT, CAP_PASSWORD, CAP_FORMATTED_PREEDIT = 1 << 1, 1 << 3, 1 << 4
CAP_SENSITIVE, CAP_CLIENT_SIDE_INPUT_PANEL = 1 << 36, 1 << 39
CAP_DEFAULT = CAP_PREEDIT | CAP_FORMATTED_PREEDIT | CAP_CLIENT_SIDE_INPUT_PANEL
# fcitx-utils/keysym.h (KeyState)
SHIFT, CAPS, CTRL, ALT = 1, 2, 4, 8
# fcitx-utils/keysymgen.h
RETURN, ESC, BACKSPACE, TAB, SPACE = 0xff0d, 0xff1b, 0xff08, 0xff09, 0x20
LEFT, UP, RIGHT, DOWN, PAGE_UP, PAGE_DOWN = 0xff51, 0xff52, 0xff53, 0xff54, 0xff55, 0xff56
SHIFT_L, SHIFT_R, CTRL_L = 0xffe1, 0xffe2, 0xffe3


class Fx:
    def __init__(self, wait_s=10.0):
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION)
        self.loop_ctx = GLib.MainContext.default()
        self.contexts = []
        end = time.monotonic() + wait_s
        while True:
            try:
                self.call("/controller", "org.freedesktop.DBus.Peer", "Ping", None)
                break
            except GLib.Error:
                if time.monotonic() > end:
                    raise
                time.sleep(0.05)

    def call(self, path, iface, method, args, timeout_ms=5000):
        return self.bus.call_sync(SERVICE, path, iface, method, args, None, Gio.DBusCallFlags.NO_AUTO_START, timeout_ms, None)

    def pump(self, settle_s=0.0):
        """Dispatches the signals that have arrived; with settle_s, keeps going until that long has passed quietly."""
        end = time.monotonic() + settle_s
        while True:
            if self.loop_ctx.iteration(False):
                end = time.monotonic() + settle_s
            elif time.monotonic() >= end:
                return
            else:
                time.sleep(0.002)

    def controller(self, method, args=None):
        return self.call("/controller", "org.fcitx.Fcitx.Controller1", method, args)

    def context(self, caps=CAP_DEFAULT, program="twpotato-test"):
        self.contexts.append(Ctx(self, caps, program))
        return self.contexts[-1]

    def close(self):
        """Stops listening for every context made here (the next fcitx5 uses the same object paths again)."""
        for c in self.contexts:
            c.unsubscribe()


class Ctx:
    """One input context. Everything the server sent is kept in `events` (name, unpacked arguments) in arrival order;
    commits / preedit / cands hold the latest state for convenience."""

    def __init__(self, fx, caps, program):
        self.fx = fx
        path, _uuid = fx.call("/org/freedesktop/portal/inputmethod", "org.fcitx.Fcitx.InputMethod1", "CreateInputContext",
                              GLib.Variant("(a(ss))", ([("program", program)],))).unpack()
        self.path = path
        self.events = []
        self.commits = []          # every committed string
        self.preedit = ("", 0)     # the preedit in the client: text, cursor in UTF-8 bytes
        self.panel_preedit = ("", -1)  # the preedit of the panel (used when the client has no preedit capability)
        self.cands = None          # None = no candidates; else dict(items=[(label, text)], index, has_prev, has_next)
        self.aux = ""
        self.forwarded = []        # (key symbol, states, is a release) of every key the input method sent on
        self.sub = fx.bus.signal_subscribe(SERVICE, "org.fcitx.Fcitx.InputContext1", None, path, None,
                                           Gio.DBusSignalFlags.NONE, self._on_signal)
        self.set_caps(caps)

    def _ic(self, method, args=None, timeout_ms=5000):
        return self.fx.call(self.path, "org.fcitx.Fcitx.InputContext1", method, args, timeout_ms)

    def _on_signal(self, _bus, _sender, _path, _iface, name, params):
        a = params.unpack()
        self.events.append((name, a))
        if name == "CommitString":
            self.commits.append(a[0])
        elif name == "ForwardKey":
            self.forwarded.append(tuple(a))
        elif name == "UpdateFormattedPreedit":
            self.preedit = ("".join(s for s, _ in a[0]), a[1])
        elif name == "UpdateClientSideUI":
            preedit, cursor, aux_up, _aux_down, cands, index, _layout, has_prev, has_next = a
            self.panel_preedit = ("".join(s for s, _ in preedit), cursor)
            self.aux = "".join(s for s, _ in aux_up)
            self.cands = dict(items=list(cands), index=index, has_prev=has_prev, has_next=has_next) if cands else None

    def set_caps(self, caps):
        self.caps = caps
        self._ic("SetCapability", GLib.Variant("(t)", (caps,)))
        self.fx.pump()

    def focus_in(self):
        self._ic("FocusIn"); self.fx.pump()

    def focus_out(self):
        self._ic("FocusOut"); self.fx.pump()

    def reset(self):
        self._ic("Reset"); self.fx.pump()

    def key(self, keysym, state=0, release=False, keycode=0):
        """One key event; returns (handled, seconds the D-Bus call took). Signals sent before the reply are dispatched."""
        t = time.perf_counter()
        (handled,) = self._ic("ProcessKeyEvent", GLib.Variant("(uuubu)", (keysym, keycode, state, release, 0))).unpack()
        dt = time.perf_counter() - t
        self.fx.pump()
        return handled, dt

    def tap(self, keysym, state=0, keycode=0):
        h, dt = self.key(keysym, state, keycode=keycode)
        self.key(keysym, state, release=True, keycode=keycode)
        return h, dt

    def select(self, index):
        self._ic("SelectCandidate", GLib.Variant("(i)", (index,))); self.fx.pump()

    def page(self, forward):
        self._ic("NextPage" if forward else "PrevPage"); self.fx.pump()

    def unsubscribe(self):
        if self.sub is not None:
            self.fx.bus.signal_unsubscribe(self.sub)
            self.sub = None

    def destroy(self):
        self.unsubscribe()
        self._ic("DestroyIC")


# ---- the processes of this run (run.sh puts TWFX_STAGE into the environment of everything it starts) ----

def stage_procs(name):
    """Process ids of the live (not zombie) processes of this run with this command name."""
    pids = []
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            stat = open(f"/proc/{d}/stat").read()
            env = open(f"/proc/{d}/environ", "rb").read().split(b"\0")
        except OSError:
            continue
        comm, state = stat[stat.index("(") + 1:stat.rindex(")")], stat[stat.rindex(")") + 2]
        if comm == name and state != "Z" and f"TWFX_STAGE={STAGE}".encode() in env:
            pids.append(int(d))
    return pids


def engines():
    return stage_procs("twime-engined")


def kill_engines(wait_s=10.0):
    end = time.monotonic() + wait_s
    while True:
        pids = engines()
        if not pids:
            return
        for p in pids:
            try:
                os.kill(p, signal.SIGKILL)
            except ProcessLookupError:
                pass
        assert time.monotonic() < end, "engine does not end"
        time.sleep(0.01)


def engine_status():
    """(open sessions, sessions composing, connections that ended without closing their sessions), asked from the
    engine itself over its socket (twime-proto: a request of session 0 with the event Status)."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(2.0)
    s.connect(os.path.join(os.environ["XDG_RUNTIME_DIR"], "twpotato", "engine.sock"))
    payload = struct.pack("<BIIBB", 1, 0, 0, 0, 11)
    s.sendall(struct.pack("<I", len(payload)) + payload)
    buf = b""
    while len(buf) < 4 or len(buf) < 4 + struct.unpack("<I", buf[:4])[0]:
        chunk = s.recv(64)
        assert chunk, "engine closed the connection"
        buf += chunk
    s.close()
    kind, sessions, composing, abnormal = struct.unpack("<BHHH", buf[4:11])
    assert kind == 5, buf
    return sessions, composing, abnormal


class Fcitx:
    """A headless fcitx5 on the session bus of this process. Its output (and the addon's lines) go to the log file.
    `enable`: addons of the default disable list to leave on (kimpanel for the status display, xcb with a DISPLAY).
    `disable`: more addons to turn off."""

    def __init__(self, log_path, enable=(), env=None, disable=()):
        assert not stage_procs("fcitx5"), "an fcitx5 of this run is still there"
        off = [a for a in ("wayland", "xcb", "waylandim", "xim", "kimpanel", "notificationitem") if a not in enable] + list(disable)
        self.log = open(log_path, "ab")
        self.p = subprocess.Popen(["fcitx5", "--disable=" + ",".join(off)], stdin=subprocess.DEVNULL, stdout=self.log,
                                  stderr=subprocess.STDOUT, env=dict(os.environ, **(env or {})))
        self.fx = Fx()

    def stop(self):
        """Ends fcitx5 (it must go by itself on SIGTERM) and the engine it left behind."""
        self.p.terminate()
        try:
            code = self.p.wait(10)
        except subprocess.TimeoutExpired:
            self.p.kill()
            raise AssertionError("fcitx5 did not end on SIGTERM")
        finally:
            self.fx.close()
            self.log.close()
            kill_engines()
        return code


class Kim:
    """Stands in for a desktop panel that speaks the kimpanel protocol: fcitx5's kimpanel addon sends it the status
    area, which is where the Chinese / English indicator of the input method shows. Create it before Fcitx(enable=
    ("kimpanel",)). `status()` is the last input method property received, e.g. '/Fcitx/im:地瓜輸入法::英文:menu,label=英'."""

    def __init__(self):
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION)
        self.props = []
        self.sub = self.bus.signal_subscribe(None, "org.kde.kimpanel.inputmethod", None, None, None, Gio.DBusSignalFlags.NONE, self._on_signal)
        self.name = Gio.bus_own_name_on_connection(self.bus, "org.kde.impanel", Gio.BusNameOwnerFlags.NONE, None, None)

    def close(self):
        self.bus.signal_unsubscribe(self.sub)
        Gio.bus_unown_name(self.name)

    def _on_signal(self, _bus, _sender, _path, _iface, name, params):
        a = params.unpack()
        if name == "UpdateProperty" and a[0].startswith("/Fcitx/im:"):
            self.props.append(a[0])
        elif name == "RegisterProperties":
            self.props.extend(p for p in a[0] if p.startswith("/Fcitx/im:"))

    def status(self):
        return self.props[-1] if self.props else ""


def write_fcitx_config(stock_hotkeys=False):
    """fcitx5's own settings for the next Fcitx(): new input contexts start with the input method active. Unless
    `stock_hotkeys`, Fcitx5's hotkey on a left Shift tap (AltTriggerKeys, 'temporarily switch to the first input
    method') is taken away, so that both Shift keys reach the input method as taps."""
    text = "[Behavior]\nActiveByDefault=True\nShareInputState=No\n"
    if not stock_hotkeys:
        text += "\n[Hotkey]\nAltTriggerKeys=\n"
    with open(os.path.join(os.environ["XDG_CONFIG_HOME"], "fcitx5", "config"), "w") as f:
        f.write(text)
