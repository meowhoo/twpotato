"""Assertions 3 to 7 of LINUX-plan 7.2 on IBus, plus a few basics of the shell. The same checks as
fcitx5/test/t_func.py wherever the two frameworks allow it; what differs is said at the test. Every test has its own
ibus-daemon (and with it its own ibus-engine-twpotato and engine) and its own process; run by main.py."""
import os, signal, time

from ib import *
from keysamples import OWN, SAMPLES
from zhuyin import keys_of_line

CALL_TIMEOUT_MS = 30  # twime-shell: time limit of one event
LOG = os.path.join(STAGE, "ibus.log")
STARTS = os.path.join(STAGE, "engine-starts")  # one line per start of the engine (the wrapper run.sh installs)
ASSETS_LINK = os.path.join(os.environ["XDG_DATA_HOME"], "twpotato", "assets")


def note(text):
    print("    " + text)


def starts():
    return [float(l) for l in open(STARTS)] if os.path.exists(STARTS) else []


def ready(ic, limit_s=10.0, since=None):
    """Presses a key the engine never gets until the shell has its connection and a session; the seconds it took,
    counted from `since` (default: now). Fails unless that is less than limit_s, also when the last attempt succeeds."""
    t = time.monotonic() if since is None else since
    while True:
        h, _ = ic.tap(CTRL_L)
        assert not h
        try:
            up = engine_status()[0] >= 1
        except OSError:
            up = False
        took = time.monotonic() - t
        assert took < limit_s, f"no engine within {limit_s} s" if not up else f"the engine took {took:.2f} s (limit {limit_s} s)"
        if up:
            return took
        time.sleep(0.02)


def focus_engine(d, ic):
    """Gives the context the focus and our engine, and waits until the engine object has taken it (with a shared
    engine object the focus alone brings it; asking for the engine again does no harm)."""
    seen = len(d.ib.panel.modes)
    ic.focus_in()
    ic.choose_engine(ENGINE)
    end = time.monotonic() + 5
    while len(d.ib.panel.modes) == seen:
        assert time.monotonic() < end, "the engine did not take the context"
        d.ib.pump(0.01)


def wait(d, cond, what, limit_s=5.0):
    """Dispatches what arrives until cond() holds; fails after limit_s."""
    end = time.monotonic() + limit_s
    while not cond():
        assert time.monotonic() < end, f"timeout: {what}"
        d.ib.pump(0.01)


def attach(d, caps=CAP_DEFAULT, purpose=None):
    """A new input context with the focus and our engine on it (the engine process is not asked for yet)."""
    ic = d.ib.context(caps)
    if purpose is not None:
        ic.set_purpose(*purpose)
    focus_engine(d, ic)
    return ic


def context(d, caps=CAP_DEFAULT):
    ic = attach(d, caps)
    ready(ic)
    return ic


def refocus(d, ic):
    """Focus on a context that had it before; waits until the engine object is there again."""
    seen = len(d.ib.panel.modes)
    ic.focus_in()
    end = time.monotonic() + 5
    while len(d.ib.panel.modes) == seen:
        assert time.monotonic() < end, "the engine did not get the focus"
        d.ib.pump(0.01)


def typ(ic, keys):
    """Types keys of the standard layout; every one must be taken by the input method."""
    for k in keys:
        h, _ = ic.tap(ord(k))
        assert h, f"key {k!r} was not handled"


def passes(d, ic, keys="su3cl3"):
    """These keys (and Enter, Space) must go to the application and leave no trace."""
    commits, panel = len(ic.commits), d.ib.panel
    for sym in [ord(k) for k in keys] + [RETURN, SPACE]:
        h, _ = ic.tap(sym)
        assert not h, f"key {sym:#x} was handled"
        assert ic.preedit == ("", 0) and panel.preedit[0] == "" and panel.cands is None and len(ic.commits) == commits, (ic.preedit, panel.cands, ic.commits)


def text(ic):
    return ic.preedit[0]


# ---- basics --------------------------------------------------------------------------------------------------

def t_basic_preedit_cursor_commit(d):
    ic = context(d)
    typ(ic, "su3cl3")
    assert ic.preedit == ("你好", 2), ic.preedit  # the caret is in characters
    kind, preedit, mode, attrs = ic.events[-1]
    assert (1, 1, 0, 2) in attrs, f"the preedit is underlined: {attrs}"  # IBUS_ATTR_TYPE_UNDERLINE, single, characters 0..2
    for want in (1, 0):
        h, _ = ic.tap(LEFT)
        assert h and ic.preedit == ("你好", want), ic.preedit
    h, _ = ic.tap(RETURN)
    assert h and ic.commits == ["你好"] and ic.preedit == ("", 0), (ic.commits, ic.preedit)
    # Nothing composing: Enter belongs to the application.
    h, _ = ic.tap(RETURN)
    assert not h and ic.commits == ["你好"]
    # Ctrl combinations are never taken, composing or not.
    typ(ic, "su3")
    for sym in (ord("c"), ord("a")):
        h, _ = ic.tap(sym, CTRL)
        assert not h
    assert text(ic) == "你"


def t_basic_client_without_preedit(d):
    """A client that cannot show a preedit gets it in the panel instead."""
    ic = context(d, CAP_FOCUS)
    typ(ic, "su3cl3")
    assert d.ib.panel.preedit == ("你好", 2) and ic.preedit == ("", 0), (d.ib.panel.preedit, ic.preedit)
    ic.tap(RETURN)
    assert ic.commits == ["你好"] and d.ib.panel.preedit[0] == ""


def t_basic_reset_and_switching_away(d):
    """IBus hands the preedit to the client with a rule for the moment the input method is no longer asked: commit it
    or drop it. The shell sets the rule from what the engine would do on a focus loss: a composition that can be
    committed is committed, bopomofo still being typed is dropped. That rule also applies to a reset from the
    application - where Fcitx5 drops every composition."""
    ic = context(d)
    typ(ic, "su3")
    ic.reset()
    assert ic.preedit == ("", 0) and ic.commits == ["你"] and engine_status()[1] == 0, (ic.preedit, ic.commits)
    typ(ic, "s")  # not committable
    assert text(ic) == "ㄋ"
    ic.reset()
    assert ic.preedit == ("", 0) and ic.commits == ["你"] and engine_status()[1] == 0, (ic.preedit, ic.commits)
    typ(ic, "cl3")
    assert text(ic) == "好"
    # The user switches to another input method: the composition is committed, as when the focus leaves.
    ic.set_engine(KEYBOARD)
    assert ic.preedit == ("", 0) and ic.commits == ["你", "好"], (ic.preedit, ic.commits)
    h, _ = ic.tap(ord("s"))
    assert not h
    ic.set_engine(ENGINE)
    ready(ic)
    typ(ic, "su3")
    ic.tap(RETURN)
    assert ic.commits == ["你", "好", "你"], ic.commits
    # A composition that cannot be committed is dropped by the switch.
    typ(ic, "s")
    ic.set_engine(KEYBOARD)
    d.ib.settle()
    assert ic.preedit == ("", 0) and ic.commits == ["你", "好", "你"], (ic.preedit, ic.commits)


def t_basic_keys_by_their_place_on_the_keyboard(d):
    """The bopomofo layout belongs to the places of the keys. A client that sends key codes may run under another
    system layout: here the key values are the ones Dvorak gives, the codes those of the physical keys S, U, 3, C, L, 3
    (evdev numbering). Without a code (as everywhere else in these tests) the value is taken as it comes."""
    ic = context(d)
    for value, code in (("o", 31), ("g", 22), ("3", 4), ("j", 46), ("n", 38), ("3", 4)):
        h, _ = ic.tap(ord(value), keycode=code)
        assert h
    assert text(ic) == "你好", ic.preedit
    h, _ = ic.tap(RETURN, keycode=28)
    assert h and ic.commits == ["你好"]
    # Shift with a key: the place decides, the modifier is passed on (Dvorak's Shift+S is "O").
    typ(ic, "su3")
    h, _ = ic.tap(ord("O"), SHIFT, keycode=31)
    by_place = (h, ic.preedit, list(ic.commits))
    ic.tap(ESC); ic.tap(ESC); ic.reset(); ic.commits[:] = ["你好"]
    typ(ic, "su3")
    h, _ = ic.tap(ord("S"), SHIFT)
    assert by_place == (h, ic.preedit, list(ic.commits)) and h, (by_place, ic.preedit, ic.commits)
    ic.tap(ESC); ic.tap(ESC); ic.reset(); ic.commits[:] = ["你好"]
    # The same values without codes are other keys: o is ㄟ, g is ㄕ.
    typ(ic, "og")
    assert text(ic) != "ㄋㄧ" and text(ic), ic.preedit
    ic.tap(ESC)
    # A key that does not type text keeps its meaning whatever code comes with it (a client with made-up codes, a
    # remapped keyboard): Escape with the code of the left bracket key is Escape.
    ic.tap(ESC)
    typ(ic, "su3")
    h, _ = ic.tap(ESC, keycode=26)
    assert h and ic.preedit == ("", 0) and ic.commits == ["你好"], (ic.preedit, ic.commits)
    # A text key with a code outside the table (here the extra key of ISO keyboards) is what its symbol says; and a
    # symbol of a layout without Latin letters (Cyrillic yeru, the S key of the Russian layout) is placed by its code.
    h, _ = ic.tap(ord("s"), keycode=86)
    assert h and text(ic) == "ㄋ", ic.preedit
    ic.tap(ESC)
    h, _ = ic.tap(0x6d9, keycode=31)
    assert h and text(ic) == "ㄋ", ic.preedit
    ic.tap(ESC)
    # The number pad keeps its meaning (Num Lock decides it, not the layout): a digit there is not the bopomofo key.
    h, _ = ic.tap(0xffb3, keycode=81)  # KP_3
    assert "ˇ" not in text(ic), ic.preedit


def t_basic_which_keys_are_placed_by_their_key_code(d):
    """The same list of key symbols as in the other framework's test (keysamples.py), each sent with the key code of
    the S key: a symbol that types text is the S key (ㄋ); every other symbol keeps its own meaning."""
    ic = context(d)
    for name, sym, is_text in SAMPLES:
        assert ic.preedit == ("", 0), (name, ic.preedit)
        h, _ = ic.tap(sym, keycode=31)
        if is_text:
            assert h and text(ic) == "ㄋ", f"{name} ({sym:#x}) with the code of S: handled {h}, preedit {ic.preedit}"
            ic.tap(ESC)
        else:
            assert text(ic) == "" and ic.commits == OWN.get(name, []), f"{name} ({sym:#x}) with the code of S: preedit {ic.preedit}, commits {ic.commits}"
            ic.commits.clear()


# ---- 3: candidates -------------------------------------------------------------------------------------------

def t3_candidates(d):
    """The lookup table goes to the panel (the test). Every table holds the page the engine gave and nothing else, its
    cursor is the engine's highlight; paging is the engine's, from the keys and from the panel's signals alike."""
    ic, panel = context(d), d.ib.panel
    typ(ic, "g4")  # ㄕˋ: more than two pages of candidates
    first = text(ic)
    h, _ = ic.tap(DOWN)
    assert h and panel.cands, "Down opens the list"
    p1 = panel.cands
    assert [l for l, _ in p1["items"]] == [str(i) for i in range(1, 10)], p1
    assert p1["index"] == 0 and p1["items"][0][1] == first and p1["page_size"] == len(p1["items"]) == 9, p1
    assert text(ic) == first and ic.commits == []
    # The highlight follows the engine: keys, and the panel's cursor signals.
    ic.tap(DOWN)
    assert panel.cands == dict(p1, index=1), panel.cands
    ic.tap(UP)
    assert panel.cands == p1
    panel.cursor_down(); d.ib.settle()
    assert panel.cands == dict(p1, index=1), panel.cands
    panel.cursor_up(); d.ib.settle()
    assert panel.cands == p1
    # Paging from the panel (what a click on its arrows does) and by the keys gives the same pages.
    panel.page_down(); d.ib.settle()
    p2 = panel.cands
    assert p2 and p2["index"] == 0 and p2["page_size"] == len(p2["items"]) == 9, p2
    assert not {t for _, t in p1["items"]} & {t for _, t in p2["items"]}, "the second page has other candidates"
    panel.page_up(); d.ib.settle()
    assert panel.cands == p1
    h, _ = ic.tap(PAGE_DOWN)
    assert h and panel.cands == p2
    panel.page_down(); d.ib.settle()
    p3 = panel.cands
    assert p3["items"] != p2["items"]
    h, _ = ic.tap(PAGE_UP)
    assert h and panel.cands == p2
    # A digit picks from the page shown.
    h, _ = ic.tap(ord("3"))
    assert h and panel.cands is None and text(ic) == p2["items"][2][1] and ic.commits == [], (panel.cands, ic.preedit)
    # A click on a candidate.
    ic.tap(DOWN)
    page = panel.cands
    panel.candidate_clicked(4, 1, 0); d.ib.settle()
    assert panel.cands is None and text(ic) == page["items"][4][1] and ic.commits == [], (panel.cands, ic.preedit)
    picked = text(ic)
    # Esc closes the list and nothing else.
    ic.tap(DOWN)
    assert panel.cands
    h, _ = ic.tap(ESC)
    assert h and panel.cands is None and text(ic) == picked
    ic.tap(RETURN)
    assert ic.commits == [picked] and ic.preedit == ("", 0)
    # Word candidates of a longer composition, and a list of one page.
    typ(ic, "su3cl3")
    ic.tap(DOWN)
    assert panel.cands and panel.cands["items"][0] == ("1", "你好") and panel.cands["page_size"] == len(panel.cands["items"]) < 9, panel.cands
    panel.candidate_clicked(1, 1, 0); d.ib.settle()
    second = text(ic)
    ic.tap(RETURN)
    assert ic.commits == [picked, second] and second != "你好"


# ---- 4: Shift tap, status display ------------------------------------------------------------------------------

def shift_tap(ic, sym):
    h1, _ = ic.key(sym)
    h2, _ = ic.key(sym, SHIFT, release=True)
    assert not h1 and not h2, "a Shift press and release always go to the application"


def t4_shift_tap_and_status(d):
    """The indicator is an IBus property; the panel (the test) receives it and can click it."""
    ic, panel = context(d), d.ib.panel
    assert panel.modes[-1] == ("中", "中文"), panel.modes
    shift_tap(ic, SHIFT_L)
    assert panel.modes[-1] == ("英", "英文"), panel.modes
    passes(d, ic)
    shift_tap(ic, SHIFT_R)
    assert panel.mode() == "中"
    typ(ic, "su3")
    assert text(ic) == "你"
    ic.tap(RETURN)
    # Not a tap: Shift held with a letter, and Shift held long.
    n = len(panel.modes)
    ic.key(SHIFT_L)
    ic.key(ord("A"), SHIFT)
    ic.key(ord("A"), SHIFT, release=True)
    ic.key(SHIFT_L, SHIFT, release=True)
    ic.tap(ESC)
    ic.key(SHIFT_R)
    time.sleep(0.6)
    ic.key(SHIFT_R, SHIFT, release=True)
    d.ib.settle()
    assert all(m[0] == "中" for m in panel.modes[n:]), panel.modes[n:]
    typ(ic, "cl3")
    assert text(ic) == "好"
    ic.tap(RETURN)
    assert ic.commits == ["你", "好"], ic.commits
    # A click on the indicator switches too.
    panel.property_activate("InputMode", 0); d.ib.settle()
    assert panel.mode() == "英"
    passes(d, ic)
    panel.property_activate("InputMode", 0); d.ib.settle()
    assert panel.mode() == "中"
    typ(ic, "su3")
    assert text(ic) == "你"
    # With Caps Lock on a click does not switch - also after the panel's own buttons were used (they are sent to the
    # engine as keys and must not make the shell forget Caps Lock).
    ic.tap(DOWN, CAPS)
    assert panel.cands
    panel.cursor_down()
    wait(d, lambda: panel.cands["index"] == 1, "the highlight moves")  # no key in between: it would repair Caps Lock
    n = len(panel.modes)
    panel.property_activate("InputMode", 0); d.ib.settle()
    assert panel.modes[n:] == [] and panel.mode() == "中", panel.modes[n:]


# ---- 5: password, PIN and private fields ------------------------------------------------------------------------
# The client tells the purpose of the field and its hints (set_content_type); the shell tells the engine as
# TWIME_EV_SENSITIVE for the purposes password and PIN, and for the hint "private" on a field of any purpose.
FIELDS = (("password", (PURPOSE_PASSWORD, 0)), ("pin", (PURPOSE_PIN, 0)), ("private", (PURPOSE_FREE, HINT_PRIVATE)))


def t5_field_marked_from_the_start(d):
    context(d).focus_out()  # the engine is up
    for name, field in FIELDS:
        ic = attach(d, purpose=field)
        passes(d, ic)
        assert ic.commits == [], name
        ic.focus_out()
        ic.destroy()
    assert engine_status()[1] == 0


def t5_ordinary_to_sensitive_and_back_without_a_focus_change(d):
    for name, field in FIELDS:
        ic = context(d)
        typ(ic, "su3")
        assert text(ic) == "你"
        ic.set_purpose(*field)
        # No key in between: the change alone must end the composition (a key would make the shell look again).
        wait(d, lambda: ic.preedit == ("", 0) and engine_status()[1] == 0, f"{name}: the composition ends at the change")
        assert d.ib.panel.cands is None and ic.commits == [], (name, ic.commits)
        passes(d, ic)
        ic.set_purpose(PURPOSE_FREE)
        d.ib.settle()
        typ(ic, "cl3")
        assert text(ic) == "好", name
        ic.tap(RETURN)
        assert ic.commits == ["好"], (name, ic.commits)
        ic.focus_out()
        ic.destroy()


# (text committed before, word typed after): without the earlier text the word converts differently.
CONTEXT_PAIRS = (("ㄌㄢˊ ㄑㄧㄡˊ ㄅㄧˇ ㄙㄞˋ", "ㄍㄨㄥ ㄕˋ"), ("ㄧㄣ ㄩㄝˋ ㄏㄨㄟˋ", "ㄒㄧㄥˊ ㄕˋ"), ("ㄧ ㄕㄥ ㄕㄨㄛ", "ㄧㄡˊ ㄩˊ"),
                 ("ㄉㄧㄢˋ ㄋㄠˇ ㄔㄥˊ ㄕˋ", "ㄏㄨㄟˋ ㄏㄨㄚˋ"))


def earlier_text(d, clear, what):
    """After `clear(ic)` the engine must not remember what was typed before. Control: the same word converts
    differently when the earlier text is remembered, so a cleared memory shows."""
    ic = context(d)

    def word(z):
        typ(ic, "".join(keys_of_line(z)))
        got = text(ic)
        ic.tap(ESC)
        assert ic.preedit == ("", 0)
        return got

    def sentence(z):
        typ(ic, "".join(keys_of_line(z)))
        ic.tap(RETURN)

    used = 0
    for before, after in CONTEXT_PAIRS:
        ic.focus_out(); refocus(d, ic)
        alone = word(after)
        ic.focus_out(); refocus(d, ic)
        sentence(before)
        remembered = word(after)
        if remembered == alone:
            note(f"context pair without effect on this model (not used): {ic.commits[-1]} + {alone}")
            continue
        used += 1
        for name in clear(None):
            ic.focus_out(); refocus(d, ic)
            sentence(before)
            assert ic.preedit == ("", 0)
            clear((ic, name))
            cleared = word(after)
            assert cleared == alone, f"{name}: after {ic.commits[-1]} the word is {cleared}, with empty memory {alone}, with memory {remembered}"
        note(f"context control: after {ic.commits[-1]!r} the word is {remembered!r}; alone and after {what} {alone!r}")
    assert used >= 2, "the control pairs no longer depend on the earlier text: choose new ones"


def t5_sensitive_field_clears_the_earlier_text(d):
    def clear(arg):
        if arg is None:
            return [name for name, _ in FIELDS]
        ic, name = arg
        ic.set_purpose(*dict(FIELDS)[name])
        d.ib.settle()
        passes(d, ic)
        ic.set_purpose(PURPOSE_FREE)
        d.ib.settle()

    earlier_text(d, clear, "a password / PIN / private state")


def t5_reset_without_a_composition_clears_the_earlier_text(d):
    """A reset from the application (the caret went elsewhere) while nothing is being composed."""
    def clear(arg):
        if arg is None:
            return ["reset"]
        arg[0].reset()

    earlier_text(d, clear, "a reset")


def engine_ends_under_a_sensitive_field(d, name):
    ic = context(d)
    typ(ic, "su3")
    ic.set_purpose(*dict(FIELDS)[name])
    d.ib.settle()
    passes(d, ic)
    first = engines()
    assert len(first) == 1
    kill_engines()
    t = time.monotonic()
    passes(d, ic)  # the first of these keys finds the connection dead and starts another engine
    while True:
        passes(d, ic, "s")
        try:
            if engines() and engines() != first and engine_status()[0] == 1:
                break
        except OSError:
            pass
        assert time.monotonic() - t < 10, "the engine did not come back"
        time.sleep(0.05)
    took = time.monotonic() - t
    assert took < 10, f"the engine came back after {took:.2f} s"
    note(f"{name} field, engine ended: a new engine and session after {took:.2f} s")
    passes(d, ic)
    assert engine_status()[:2] == (1, 0), "a new session exists and nothing was composed in it"
    assert ic.commits == []
    ic.set_purpose(PURPOSE_FREE)
    d.ib.settle()
    typ(ic, "su3")
    assert text(ic) == "你" and engine_status()[:2] == (1, 1)


def t5_engine_ends_while_a_password_field_has_the_focus(d):
    engine_ends_under_a_sensitive_field(d, "password")


def t5_engine_ends_while_a_private_field_has_the_focus(d):
    engine_ends_under_a_sensitive_field(d, "private")


# ---- 6: two input contexts -------------------------------------------------------------------------------------
# A session belongs to an engine object, and ibus-daemon 1.5.29 shares one engine object between all input contexts.
# (Older versions had a setting for an engine object per context; this version's daemon has no such setting, so that
# arrangement cannot be tested here. It would be the plain case: one context, one object, one session.)

def t6_two_contexts_sharing_one_engine_object(d):
    a = context(d)
    b = d.ib.context()
    typ(a, "su3")
    assert text(a) == "你"
    # Leaving a context: a composition that can be committed is committed there, once.
    a.focus_out()
    d.ib.settle()
    assert a.preedit == ("", 0) and a.commits == ["你"], (a.preedit, a.commits)
    refocus(d, b)
    typ(b, "cl3")
    assert engine_status()[:2] == (1, 1), "one engine object, one session"
    assert text(b) == "好" and b.commits == []
    b.tap(RETURN)
    assert b.commits == ["好"]
    b.focus_out()
    refocus(d, a)
    typ(a, "cl3")
    a.tap(RETURN)
    assert a.commits == ["你", "好"] and b.commits == ["好"], (a.commits, b.commits)
    a.focus_out()
    # Syllable by syllable in turns: every focus loss ends the composition of that moment in its own context.
    for ka, kb in (("su3", "cl3"), ("cl3", "su3")):
        refocus(d, a); typ(a, ka); a.focus_out()
        refocus(d, b); typ(b, kb); b.focus_out()
    d.ib.settle()
    assert a.commits[2:] == ["你", "好"] and b.commits[1:] == ["好", "你"], (a.commits, b.commits)
    # Bopomofo still being typed is dropped with the focus and does not show up in the other context.
    refocus(d, a); typ(a, "s"); a.focus_out()
    refocus(d, b)
    assert b.preedit == ("", 0) and engine_status()[1] == 0
    typ(b, "cl3"); b.tap(RETURN)
    assert a.commits[2:] == ["你", "好"] and b.commits[1:] == ["好", "你", "好"], (a.commits, b.commits)
    # The session belongs to the engine object, which outlives a context that shared it.
    b.focus_out(); b.destroy()
    refocus(d, a)  # the engine object has handled everything before this
    assert engine_status()[:2] == (1, 0), engine_status()


# ---- 6b: the client is gnome-shell (GNOME on Wayland; LINUX-plan 7.7) --------------------------------------------

F6, ALT_L = 0xffc3, 0xffe9


def gnome_shell(d):
    """A context of the client gnome-shell. The engine learns the client name only from a focus change (focus-in-id);
    the focus-in that puts the engine on an already focused context has none."""
    ic = d.ib.context(name="gnome-shell")
    focus_engine(d, ic)
    ready(ic)
    ic.focus_out()
    refocus(d, ic)
    return ic


def t6b_gnome_shell_composition_is_committed_before_a_key_that_passes(d):
    """F6, Ctrl+L, Alt+Tab ... may move the focus once the client has the key: the composition is committed first, so
    that it arrives in the field it was typed in. The client is never told to commit it itself."""
    ic = gnome_shell(d)
    typ(ic, "su3cl3")
    assert text(ic) == "你好"
    h, _ = ic.tap(F6)
    assert not h and ic.commits == ["你好"] and ic.preedit == ("", 0)
    # Alt alone does nothing to the composition; the Tab of Alt+Tab, held with Alt, commits it.
    typ(ic, "cl3")
    h, _ = ic.key(ALT_L)
    assert not h and ic.commits == ["你好"] and text(ic) == "好"
    h, _ = ic.key(TAB, ALT)
    assert not h and ic.commits == ["你好", "好"]
    ic.key(TAB, ALT, release=True)
    ic.key(ALT_L, ALT, release=True)
    # A bopomofo that is not finished is dropped, not committed (as on a focus loss).
    typ(ic, "su")
    ic.tap(F6)
    assert ic.commits == ["你好", "好"] and ic.preedit == ("", 0)


def t6b_gnome_shell_composition_is_dropped_on_a_focus_loss_and_a_reset(d):
    """A focus loss the input method hears of only afterwards (a mouse click, a program): nothing is committed."""
    ic = gnome_shell(d)
    typ(ic, "su3cl3")
    ic.focus_out()
    d.ib.pump(0.1)
    assert ic.commits == [], ic.commits
    refocus(d, ic)
    typ(ic, "cl3")
    assert text(ic) == "好", "the engine dropped the old composition too"
    ic.reset()
    d.ib.pump(0.1)
    assert ic.commits == [] and ic.preedit == ("", 0)
    typ(ic, "su3")
    ic.tap(RETURN)
    assert ic.commits == ["你"]


# ---- 7: faults -------------------------------------------------------------------------------------------------

def held_typing(d, ic, keys, wait_for, what):
    """Types `keys` at once (each press and release must be held: reported handled within the time limit), then waits
    for `wait_for()`. Returns (slowest key in ms, seconds until wait_for held)."""
    t = time.monotonic()
    slowest = 0.0
    for k in keys:
        for release in (False, True):
            h, dt = ic.key(ord(k), release=release)
            slowest = max(slowest, dt * 1000)
            assert h, f"{what}: key {k!r} {'release' if release else 'press'} was not held"
    wait(d, wait_for, f"{what}: commits {ic.commits} forwarded {ic.forwarded}", 3.5)
    return slowest, time.monotonic() - t


def started_at_focus(d, ic):
    """The context gets the focus and our engine; the engine process is started then (returns its pid)."""
    focus_engine(d, ic)
    end = time.monotonic() + 1
    while not engines():
        assert time.monotonic() < end, "the engine is started at the focus, not at the first key"
        time.sleep(0.005)
    return engines()[0]


def t7i_keys_typed_while_the_engine_starts_wait_for_it(d):
    """The engine is started at the focus; keys typed before it answers are held and reach it in order (LINUX-plan 7.7)."""
    assert not engines() and not starts()
    ic = d.ib.context()
    started_at_focus(d, ic)
    slowest, took = held_typing(d, ic, "su3cl3", lambda: ic.preedit[0] == "你好", "cold start")
    assert slowest < CALL_TIMEOUT_MS + 20, f"a held key took {slowest:.1f} ms"
    # The presses were the engine's; the releases go back to the application, in order.
    assert [(v, bool(st & RELEASE)) for v, _c, st in ic.forwarded] == [(ord(k), True) for k in "su3cl3"], ic.forwarded
    ic.tap(RETURN)
    assert ic.commits == ["你好"] and len(starts()) == 1 and len(engines()) == 1
    note(f"cold start: typed at once after the focus; slowest key {slowest:.1f} ms; the composition was there after {took:.2f} s")


def t7vi_held_keys_go_back_unchanged_when_the_engine_does_not_come(d):
    """The engine is started at the focus and stopped before it is ready: after 2 s the held keys go to the application
    unchanged and in order, and the next keys are not held again."""
    ic = d.ib.context()
    pid = started_at_focus(d, ic)
    os.kill(pid, signal.SIGSTOP)
    try:
        slowest, took = held_typing(d, ic, "su3", lambda: len(ic.forwarded) == 6, "engine stopped while loading")
        assert 1.9 < took < 2.5, took
        assert [(v, bool(st & RELEASE)) for v, _c, st in ic.forwarded] == [(ord(k), r) for k in "su3" for r in (False, True)], ic.forwarded
        h, dt = ic.tap(ord("s"))
        assert not h and dt * 1000 < CALL_TIMEOUT_MS + 20, "after a hold that ran out, keys pass at once"
    finally:
        os.kill(pid, signal.SIGCONT)
    ready(ic, 3.0)
    typ(ic, "su3cl3")
    ic.tap(RETURN)
    assert ic.commits == ["你好"], ic.commits
    note(f"engine stopped while loading: held keys went on unchanged after {took:.2f} s")


def t7vii_another_event_sends_held_keys_on_first(d):
    """A reset while keys are held: they go on to the application unchanged, before the reset."""
    ic = d.ib.context()
    pid = started_at_focus(d, ic)
    os.kill(pid, signal.SIGSTOP)
    try:
        assert ic.tap(ord("s"))[0]
        ic.reset()
        wait(d, lambda: len(ic.forwarded) == 2, f"held keys sent on before the reset: {ic.forwarded}")
        assert [(v, bool(st & RELEASE)) for v, _c, st in ic.forwarded] == [(ord("s"), False), (ord("s"), True)]
        # A focus loss sends them on too, but IBus 1.5.29 / 1.5.34 no longer delivers a key the engine sends after the
        # focus left (measured: the shell forwards them, the client gets nothing). They are lost; nothing else goes wrong.
        assert ic.tap(ord("u"))[0]
        ic.focus_out()
        d.ib.pump(0.2)
        assert len(ic.forwarded) == 2 and ic.commits == []
    finally:
        os.kill(pid, signal.SIGCONT)


def t7viii_no_keys_are_held_for_a_client_that_wants_them_at_once(d):
    """A client with IBUS_CAP_SYNC_PROCESS_KEY must not get forwarded keys in place of an answer: keys pass."""
    ic = d.ib.context(CAP_DEFAULT | CAP_SYNC_PROCESS_KEY)
    started_at_focus(d, ic)
    h, dt = ic.key(ord("s"))
    assert not h and dt * 1000 < CALL_TIMEOUT_MS + 20
    ic.key(ord("s"), release=True)
    ready(ic, 3.0)
    assert ic.forwarded == []


def t7ii_engine_ends_while_composing(d):
    ic = context(d)
    typ(ic, "su3")
    assert text(ic) == "你"
    kill_engines()
    t = time.monotonic()
    # The key finds the engine gone: the composition is lost, a new engine is started, and the key waits for it.
    h, dt = ic.key(ord("c"))
    assert h, "the key is held for the new engine"
    assert dt * 1000 < CALL_TIMEOUT_MS + 20, f"the key took {dt * 1000:.1f} ms"
    assert ic.preedit == ("", 0) and d.ib.panel.cands is None, "the composition on screen is removed"
    ic.key(ord("c"), release=True)
    wait(d, lambda: text(ic) == "ㄏ", "the held key reached the new engine", 3.0)
    took = time.monotonic() - t
    typ(ic, "l3")
    assert text(ic) == "好"
    ic.tap(RETURN)
    assert ic.commits == ["好"], ic.commits
    assert len(starts()) == 2
    note(f"engine ended while composing: the key returned after {dt * 1000:.1f} ms; it reached the new engine after {took:.2f} s")


def t7iii_engine_does_not_answer(d):
    """The engine is stopped (SIGSTOP) in the middle of a composition: the call runs into the shell's time limit."""
    ic = context(d)
    typ(ic, "su3")
    (pid,) = engines()
    os.kill(pid, signal.SIGSTOP)

    def stopped():
        for t in os.listdir(f"/proc/{pid}/task"):
            s = open(f"/proc/{pid}/task/{t}/stat").read()
            if s[s.rindex(")") + 2] != "T":
                return False
        return True

    try:
        while not stopped():
            time.sleep(0.001)
        h, dt = ic.key(ord("c"))
    finally:
        os.kill(pid, signal.SIGCONT)
    ms = dt * 1000
    assert not h and ic.preedit == ("", 0) and d.ib.panel.cands is None and ic.commits == []
    assert CALL_TIMEOUT_MS <= ms < CALL_TIMEOUT_MS + 20, f"the key returned after {ms:.1f} ms"
    ic.key(ord("c"), release=True)
    typ(ic, "cl3")
    assert text(ic) == "好"
    ic.tap(RETURN)
    assert ic.commits == ["好"] and len(starts()) == 1
    note(f"engine stopped: the key returned after {ms:.1f} ms (limit {CALL_TIMEOUT_MS} + 20)")


def t7iv_assets_missing(d):
    target = os.readlink(ASSETS_LINK)
    os.unlink(ASSETS_LINK)
    try:
        t = time.monotonic()
        ic = attach(d)  # the first start
        seen, slowest, taps, passed = [], 0.0, 0, 0
        while time.monotonic() - t < 7.5:
            h, dt = ic.tap(ord("s"))
            taps += 1
            passed += not h  # a key that comes while a start is still under way is held for that moment
            slowest = max(slowest, dt)
            assert ic.preedit == ("", 0)
            if len(starts()) > len(seen):
                seen.append(time.monotonic() - t)
            d.ib.pump(0.05)
    finally:
        os.symlink(target, ASSETS_LINK)
    wait(d, lambda: not engines(), "the last engine that was started has given up")
    d.ib.pump(0.1)
    # Every key reached the application once, in order: directly, or held and then sent on unchanged.
    assert passed + sum(1 for _v, _c, st in ic.forwarded if not st & RELEASE) == taps, (passed, ic.forwarded, taps)
    at = starts()
    assert len(at) == 3, f"engine starts at {seen}"
    # The shell waits 3 s between two starts; the wrapper notes its own start time a moment later (tolerance 50 ms).
    assert all(b - a >= 2.95 for a, b in zip(at, at[1:])), at
    assert not engines() and ic.commits == []
    # The engines that gave up were waited for: no zombie is left under the shell.
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                s = open(f"/proc/{p}/stat").read()
            except OSError:
                continue
            assert not ("(twime-engined)" in s and s[s.rindex(")") + 2] == "Z"), s
    note(f"assets missing: 3 engine starts in 7.5 s ({', '.join(f'{x:.2f}' for x in seen)} s); slowest key {slowest * 1000:.1f} ms")


def t7v_shell_ends_while_composing(d):
    """IBus only: the shell is a process of its own. It is killed in the middle of a composition.
    What ibus-daemon 1.5.29 does then, as observed: the context is left without an engine (keys go to the
    application), the client commits the preedit it was told to commit, and the shell is started again only when the
    engine is asked for again - not by keys, a focus change or a new context. So "typing possible again within three
    seconds" is checked from the moment the engine is selected again; nothing may be committed twice."""
    ic = context(d)
    typ(ic, "su3")
    assert text(ic) == "你"
    (shell,) = stage_procs(SHELL)
    os.kill(shell, signal.SIGKILL)
    wait(d, lambda: not stage_procs(SHELL) and ic.commits and ic.preedit == ("", 0), "the daemon has noticed the shell is gone")
    assert ic.preedit == ("", 0) and ic.commits == ["你"], f"the interrupted composition is committed once: {ic.commits} {ic.preedit}"
    for k in "su3":
        h, _ = ic.tap(ord(k))
        assert not h, "without the shell every key goes to the application"
    assert not stage_procs(SHELL) and ic.commits == ["你"]
    t = time.monotonic()
    focus_engine(d, ic)
    took = ready(ic, 3.0, since=t)
    typ(ic, "cl3")
    assert text(ic) == "好"
    ic.tap(RETURN)
    assert ic.commits == ["你", "好"], ic.commits
    assert len(stage_procs(SHELL)) == 1 and len(engines()) == 1 and len(starts()) == 1, "a new shell, the same engine"
    note(f"shell killed while composing: the client committed the composition once; after the engine was selected "
         f"again, input possible in {took:.2f} s")
