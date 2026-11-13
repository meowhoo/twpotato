"""Headless IBus client and panel for the tests, with the IBus client library (GObject introspection): the two D-Bus
roles a toolkit input module and a desktop panel play towards ibus-daemon. No display is involved.

    d = Ibus(log); ic = d.ib.context(); handled, seconds = ic.key(keyval); ic.commits, ic.preedit; d.ib.panel.cands

One process can use one ibus-daemon only (the library keeps a single bus object): main.py runs every test in a process
of its own.
"""
import collections, os, subprocess, sys, time

import gi
gi.require_version("IBus", "1.0")
from gi.repository import Gio, GLib, IBus

sys.path.append(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "fcitx5", "test"))  # after our own modules
from fx import STAGE, engine_status, engines, kill_engines, stage_procs  # noqa: E402,F401 (the processes of the run)

SHELL = "ibus-engine-twp"  # the command name of ibus-engine-twpotato as /proc shows it (15 characters)
ENGINE = "twpotato"
KEYBOARD = "xkb:us::eng"   # another engine to switch to (ibus-engine-simple)
SHIFT, CAPS, CTRL, ALT, RELEASE = 1, 2, 4, 8, 1 << 30
RETURN, ESC, BACKSPACE, TAB, SPACE = 0xff0d, 0xff1b, 0xff08, 0xff09, 0x20
LEFT, UP, RIGHT, DOWN, PAGE_UP, PAGE_DOWN = 0xff51, 0xff52, 0xff53, 0xff54, 0xff55, 0xff56
SHIFT_L, SHIFT_R, CTRL_L = 0xffe1, 0xffe2, 0xffe3
CAP_PREEDIT, CAP_FOCUS = 1 << 0, 1 << 3
CAP_DEFAULT = CAP_PREEDIT | CAP_FOCUS  # no lookup table: it goes to the panel
CAP_SYNC_PROCESS_KEY = 1 << 7  # the client wants an answer to every key, never a forwarded key in its place
PURPOSE_FREE, PURPOSE_PASSWORD, PURPOSE_PIN = 0, 8, 9
HINT_PRIVATE = 1 << 11  # IBUS_INPUT_HINT_PRIVATE: the application asks input methods not to keep what is typed
VOID = 0xffffff         # IBUS_KEY_VoidSymbol


_kept = []


def _keep(obj):
    """For the texts an input context's signals deliver (commit-text, update-preedit-text). With IBus 1.5.34 and
    PyGObject 3.56 (Ubuntu 26.04) letting go of the wrapper of such a text frees it under libibus: GLib warnings
    "assertion 'G_IS_OBJECT (object)' failed" there, a crash of this process on Arch (GLib 2.90). Found by trying,
    not by reading either library: letting the wrapper go later fails the same way, adding a reference does not help,
    never letting it go does. So these wrappers are kept until the process ends: one small object per commit and per
    preedit update, in a test process that lives for one test or one run of the cases. What the panel receives (lookup
    tables, properties, its preedit) does not need it."""
    _kept.append(obj)
    return obj


class Panel(IBus.PanelService):
    """Takes the place of the desktop's panel: receives the lookup table, the preedit of clients that cannot show
    one, and the properties (the Chinese / English indicator); sends candidate clicks, paging and property clicks."""

    def __init__(self, connection):
        super().__init__(connection=connection, object_path=IBus.PATH_PANEL)
        self.cands = None        # None = no list shown; else dict(items=[(label, text)], index, page_size)
        self.tables = 0          # lookup tables received
        self.preedit = ("", 0)
        self.modes = []          # every (symbol, label) of the indicator received, in order
        self.focus = None        # object path of the context that has the focus

    def _mode(self, prop):
        if prop.get_key() == "InputMode":
            self.modes.append((prop.get_symbol().get_text(), prop.get_label().get_text()))

    def do_update_lookup_table(self, table, visible):
        self.tables += 1
        n = table.get_number_of_candidates()
        items = [(table.get_label(i).get_text() if table.get_label(i) else None, table.get_candidate(i).get_text()) for i in range(n)]
        self._table = dict(items=items, index=table.get_cursor_pos(), page_size=table.get_page_size())
        self.cands = self._table if visible else None

    def do_show_lookup_table(self):
        self.cands = getattr(self, "_table", None)

    def do_hide_lookup_table(self):
        self.cands = None

    def do_update_preedit_text(self, text, cursor, visible):
        self.preedit = (text.get_text(), cursor) if visible else ("", 0)

    def do_hide_preedit_text(self):
        self.preedit = ("", 0)

    def do_register_properties(self, props):
        i = 0
        while props.get(i) is not None:
            self._mode(props.get(i))
            i += 1

    def do_update_property(self, prop):
        self._mode(prop)

    def do_focus_in(self, path):
        self.focus = path

    def do_focus_out(self, path):
        if self.focus == path:
            self.focus = None

    def mode(self):
        return self.modes[-1][0] if self.modes else None


class Ib:
    def __init__(self, wait_s=10.0):
        end = time.monotonic() + wait_s
        while True:  # the bus object must be made after the daemon is up: it does not find it later by itself
            try:
                address = IBus.get_address()
                if address:
                    flags = Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION
                    Gio.DBusConnection.new_for_address_sync(address, flags, None, None).close_sync(None)
                    break
            except GLib.Error:
                pass
            assert time.monotonic() < end, "no ibus-daemon"
            time.sleep(0.02)
        self.bus = IBus.Bus()
        assert self.bus.is_connected()
        self.loop_ctx = GLib.MainContext.default()
        self.contexts = []
        self.panel = Panel(self.bus.get_connection())
        assert self.bus.request_name(IBus.SERVICE_PANEL, 0) == 1, "the test panel did not get the panel's name"

    def pump(self, settle_s=0.0):
        """Dispatches what has arrived; with settle_s, keeps going until that long has passed quietly."""
        end = time.monotonic() + settle_s
        while True:
            if self.loop_ctx.iteration(False):
                end = time.monotonic() + settle_s
            elif time.monotonic() >= end:
                return
            else:
                time.sleep(0.002)

    def context(self, caps=CAP_DEFAULT, name="twpotato-test"):
        self.contexts.append(Ctx(self, caps, name))
        return self.contexts[-1]

    def settle(self):
        """Returns when everything sent so far has been handled, whichever context it concerned (Ctx.sync)."""
        for c in self.contexts:
            c.sync()


class Ctx:
    """One input context. commits / preedit hold what the client was sent (the caret counts characters)."""

    def __init__(self, ib, caps, name):
        self.ib = ib
        self.ic = ib.bus.create_input_context(name)
        self.path = self.ic.get_object_path()
        self.commits = []
        self.preedit = ("", 0)
        self.events = []
        self.forwarded = []  # (keyval, keycode, state) of every key the engine sent on
        self.lock = 0
        self.focused = False
        self.ic.connect("commit-text", self._commit)
        self.ic.connect("forward-key-event", self._forward)
        self.ic.connect("update-preedit-text", lambda _ic, text, cursor, visible: self._preedit(text, cursor, visible, None))
        self.ic.connect("update-preedit-text-with-mode", self._preedit_mode)
        self.ic.connect("hide-preedit-text", lambda _ic: self._preedit(None, 0, False, None))
        self.ic.set_capabilities(caps)
        ib.pump()

    def _commit(self, _ic, text):
        _keep(text)
        self.commits.append(text.get_text())
        self.events.append(("commit", text.get_text()))

    def _forward(self, _ic, keyval, keycode, state):
        self.forwarded.append((keyval, keycode, state))
        self.events.append(("forward", keyval, state))

    def _preedit_mode(self, _ic, text, cursor, visible, mode):
        self._preedit(text, cursor, visible, mode)

    def _preedit(self, text, cursor, visible, mode):
        _keep(text)
        self.preedit = (text.get_text(), cursor) if visible and text is not None else ("", 0)
        self.events.append(("preedit", self.preedit, mode, [(a.get_attr_type(), a.get_value(), a.get_start_index(), a.get_end_index())
                                                           for a in _attrs(text)] if text is not None else []))

    def sync(self):
        """Returns when the daemon - and the engine of the context, if it has one and the focus - has handled
        everything this process sent so far, and what they sent back has been dispatched. The calls that have no
        reply (focus, reset, content type, the panel's signals) are followed by this one, which has: a key release
        that means nothing to anybody (for a context without the focus: a question to the daemon alone, which says
        nothing about the engine). Messages of one connection are handled in the order they were sent.
        Not a neutral observer: on its way through the shell the key refreshes what the shell derives from key events
        (Caps Lock, the content type). A test of exactly those must wait for a visible state instead (t_func.wait)."""
        if self.focused:
            self.ic.process_key_event(VOID, 0, RELEASE | self.lock)
        else:  # a key would bring the focus back to the context: only the daemon is asked something
            self.ib.bus.get_use_global_engine()
        self.ib.pump()

    def set_engine(self, name, wait_s=5.0):
        """Makes this the engine of the context (the context must have the focus) and waits until it is. For our
        engine: until its engine object has the focus (it registers its indicator then)."""
        seen = len(self.ib.panel.modes)
        self.choose_engine(name)
        end = time.monotonic() + wait_s
        while (len(self.ib.panel.modes) == seen) if name == ENGINE else (self.engine() != name):
            assert time.monotonic() < end, f"the engine of the context did not become {name}"
            self.ib.pump(0.01)
        self.sync()

    def choose_engine(self, name):
        """Asks for the engine without waiting. The daemon has one engine for all contexts (4.7 of the plan), and it
        is chosen the way the desktop does it: as the global engine. Asking through the input context (SetEngine) works
        on IBus 1.5.29 but is ignored by 1.5.34 (Ubuntu 26.04), where the context keeps its engine."""
        assert self.ib.bus.set_global_engine(name), f"the daemon refused the engine {name}"

    def engine(self):
        e = self.ic.get_engine()
        return e.get_name() if e is not None else None

    def focus_in(self):
        for c in self.ib.contexts:  # the daemon takes the focus from the context that had it
            c.focused = c is self
        self.ic.focus_in(); self.sync()

    def focus_out(self):
        self.focused = False
        self.ic.focus_out(); self.sync()

    def reset(self):
        self.ic.reset(); self.sync()

    def set_purpose(self, purpose, hints=0):
        """No sync() here: its key would make the shell look at the content type by itself, and a test of what the
        change alone does could not fail. The caller waits for the state it expects."""
        self.ic.set_content_type(purpose, hints); self.ib.pump()

    def key(self, keyval, state=0, release=False, keycode=0):
        """One key event; returns (handled, seconds the synchronous call took). What was sent before the reply is
        dispatched. `keycode`: the code of the physical key (evdev numbering); 0 as clients without one send."""
        t = time.perf_counter()
        self.lock = state & CAPS  # the barrier of sync() reports Caps Lock as the last real key did
        handled = self.ic.process_key_event(keyval, keycode, state | (RELEASE if release else 0))
        dt = time.perf_counter() - t
        self.ib.pump()
        return handled, dt

    def tap(self, keyval, state=0, keycode=0):
        h, dt = self.key(keyval, state, keycode=keycode)
        self.key(keyval, state, release=True, keycode=keycode)
        return h, dt

    def destroy(self):
        self.ib.contexts.remove(self)
        self.ic.destroy(); self.ib.settle()


def _attrs(text):
    attrs = text.get_attributes()
    out, i = [], 0
    while attrs is not None and attrs.get(i) is not None:
        out.append(attrs.get(i))
        i += 1
    return out


class Ibus:
    """A headless ibus-daemon of this run: no panel of its own (the test is the panel), no configuration and emoji
    programs, no component cache. The component directory comes from IBUS_COMPONENT_PATH (run.sh). Its output and the
    shell's lines go to the log file."""

    def __init__(self, log_path, env=None):
        assert not stage_procs("ibus-daemon"), "an ibus-daemon of this run is still there"
        self.log = open(log_path, "ab")
        self.p = subprocess.Popen(["ibus-daemon", "--panel=disable", "--config=disable", "--emoji-extension=disable", "--cache=none"],
                                  stdin=subprocess.DEVNULL, stdout=self.log, stderr=subprocess.STDOUT, env=dict(os.environ, **(env or {})))
        self.ib = Ib()

    def stop(self):
        """Ends ibus-daemon (it must go on SIGTERM, and the shell with it) and the engine left behind."""
        self.p.terminate()
        try:
            code = self.p.wait(10)
        except subprocess.TimeoutExpired:
            self.p.kill()
            raise AssertionError("ibus-daemon did not end on SIGTERM")
        finally:
            self.log.close()
            end = time.monotonic() + 5
            while stage_procs(SHELL) and time.monotonic() < end:
                time.sleep(0.02)
            kill_engines()
        assert not stage_procs(SHELL), "ibus-engine-twpotato outlived ibus-daemon"
        return code
