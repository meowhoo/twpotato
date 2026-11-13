"""Assertions 3 to 7 of LINUX-plan 7.2 on Fcitx5, plus a few basics of the addon. Every test starts its own
fcitx5 (and with it its own engine); run by main.py."""
import os, signal, time

from fx import *
from keysamples import OWN, SAMPLES
from zhuyin import keys_of_line

CALL_TIMEOUT_MS = 30  # twime-shell: time limit of one event
LOG = os.path.join(STAGE, "fcitx.log")
STARTS = os.path.join(STAGE, "engine-starts")  # one line per start of the engine (the wrapper run.sh installs)
ASSETS_LINK = os.path.join(os.environ["XDG_DATA_HOME"], "twpotato", "assets")
notes = []  # numbers worth reporting, printed by main.py


def note(text):
    notes.append(text)
    print("    " + text)


def starts():
    return [float(l) for l in open(STARTS)] if os.path.exists(STARTS) else []


def ready(ic, limit_s=10.0, since=None):
    """Presses a key the engine never gets until the addon has its connection and a session; the seconds it took,
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


def context(f, caps=CAP_DEFAULT):
    ic = f.fx.context(caps)
    ic.focus_in()
    ready(ic)
    return ic


def typ(ic, keys):
    """Types keys of the standard layout; every one must be taken by the input method."""
    for k in keys:
        h, _ = ic.tap(ord(k))
        assert h, f"key {k!r} was not handled"


def passes(ic, keys="su3cl3"):
    """These keys (and Enter, Space) must go to the application and leave no trace."""
    commits = len(ic.commits)
    for sym in [ord(k) for k in keys] + [RETURN, SPACE]:
        h, _ = ic.tap(sym)
        assert not h, f"key {sym:#x} was handled"
        assert ic.preedit == ("", 0) and ic.panel_preedit[0] == "" and ic.cands is None and len(ic.commits) == commits, (ic.preedit, ic.cands, ic.commits)


def text(ic):
    return ic.preedit[0]


# ---- basics --------------------------------------------------------------------------------------------------

def t_basic_preedit_cursor_commit(f):
    ic = context(f)
    typ(ic, "su3cl3")
    assert ic.preedit == ("你好", 6), ic.preedit  # the cursor is in UTF-8 bytes
    name, args = [e for e in ic.events if e[0] == "UpdateFormattedPreedit"][-1]
    assert all(fmt & 8 for _, fmt in args[0]), "the preedit is underlined"
    for want in (3, 0):
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


def t_basic_client_without_preedit(f):
    """A client that cannot show a preedit gets it in the panel instead."""
    ic = context(f, CAP_CLIENT_SIDE_INPUT_PANEL)
    typ(ic, "su3cl3")
    assert ic.panel_preedit == ("你好", 6) and ic.preedit == ("", 0), (ic.panel_preedit, ic.preedit)
    ic.tap(RETURN)
    assert ic.commits == ["你好"] and ic.panel_preedit[0] == ""


def t_basic_reset_and_switching_away(f):
    ic = context(f)
    typ(ic, "su3")
    ic.reset()
    assert ic.preedit == ("", 0) and ic.commits == [] and engine_status()[1] == 0
    typ(ic, "cl3")
    assert text(ic) == "好"
    # The user switches to another input method: the composition is committed, as when the focus leaves.
    f.fx.controller("SetCurrentIM", GLib.Variant("(s)", ("keyboard-us",)))
    f.fx.pump(0.1)
    assert ic.preedit == ("", 0) and ic.commits == ["好"] and engine_status()[1] == 0, (ic.preedit, ic.commits)
    h, _ = ic.tap(ord("s"))
    assert not h
    f.fx.controller("SetCurrentIM", GLib.Variant("(s)", ("twpotato",)))
    f.fx.pump(0.1)
    typ(ic, "su3")
    ic.tap(RETURN)
    assert ic.commits == ["好", "你"], ic.commits
    # A composition that cannot be committed (bopomofo still being typed) is dropped by the switch.
    typ(ic, "s")
    f.fx.controller("SetCurrentIM", GLib.Variant("(s)", ("keyboard-us",)))
    f.fx.pump(0.1)
    assert ic.preedit == ("", 0) and ic.commits == ["好", "你"] and engine_status()[1] == 0, (ic.preedit, ic.commits)


def t_basic_keys_by_their_place_on_the_keyboard(f):
    """The bopomofo layout belongs to the places of the keys. A client that sends key codes may run under another
    system layout: here the key symbols are the ones Dvorak gives, the codes those of the physical keys S, U, 3, C, L, 3
    (X numbering: the Linux input event code + 8). Without a code (as everywhere else in these tests) the symbol is
    taken as it comes."""
    ic = context(f)
    for sym, code in (("o", 39), ("g", 30), ("3", 12), ("j", 54), ("n", 46), ("3", 12)):
        h, _ = ic.tap(ord(sym), keycode=code)
        assert h
    assert text(ic) == "你好", ic.preedit
    h, _ = ic.tap(RETURN, keycode=36)
    assert h and ic.commits == ["你好"]
    # Shift with a key: the place decides, the modifier is passed on (Dvorak's Shift+S is "O").
    typ(ic, "su3")
    h, _ = ic.tap(ord("O"), SHIFT, keycode=39)
    by_place = (h, ic.preedit, list(ic.commits))
    ic.tap(ESC); ic.tap(ESC); ic.reset(); ic.commits[:] = ["你好"]
    typ(ic, "su3")
    h, _ = ic.tap(ord("S"), SHIFT)
    assert by_place == (h, ic.preedit, list(ic.commits)) and h, (by_place, ic.preedit, ic.commits)
    ic.tap(ESC); ic.tap(ESC); ic.reset(); ic.commits[:] = ["你好"]
    # The same symbols without codes are other keys: o is ㄟ, g is ㄕ.
    typ(ic, "og")
    assert text(ic) != "ㄋㄧ" and text(ic), ic.preedit
    ic.tap(ESC)
    # A key that does not type text keeps its meaning whatever code comes with it (a client with made-up codes, a
    # remapped keyboard): Escape with the code of the left bracket key is Escape.
    ic.tap(ESC)
    typ(ic, "su3")
    h, _ = ic.tap(ESC, keycode=34)
    assert h and ic.preedit == ("", 0) and ic.commits == ["你好"], (ic.preedit, ic.commits)
    # A text key with a code outside the table (here the extra key of ISO keyboards) is what its symbol says; and a
    # symbol of a layout without Latin letters (Cyrillic yeru, the S key of the Russian layout) is placed by its code.
    h, _ = ic.tap(ord("s"), keycode=94)
    assert h and text(ic) == "ㄋ", ic.preedit
    ic.tap(ESC)
    h, _ = ic.tap(0x6d9, keycode=39)
    assert h and text(ic) == "ㄋ", ic.preedit
    ic.tap(ESC)
    # The number pad keeps its meaning (Num Lock decides it, not the layout): a digit there is not the bopomofo key.
    ic.tap(0xffb3, keycode=89)  # KP_3
    assert "ˇ" not in text(ic), ic.preedit


def t_basic_which_keys_are_placed_by_their_key_code(f):
    """The same list of key symbols as in the other framework's test (keysamples.py), each sent with the key code of
    the S key: a symbol that types text is the S key (ㄋ); every other symbol keeps its own meaning."""
    ic = context(f)
    for name, sym, is_text in SAMPLES:
        assert ic.preedit == ("", 0), (name, ic.preedit)
        h, _ = ic.tap(sym, keycode=39)
        if is_text:
            assert h and text(ic) == "ㄋ", f"{name} ({sym:#x}) with the code of S: handled {h}, preedit {ic.preedit}"
            ic.tap(ESC)
        else:
            assert text(ic) == "" and ic.commits == OWN.get(name, []), f"{name} ({sym:#x}) with the code of S: preedit {ic.preedit}, commits {ic.commits}"
            ic.commits.clear()


# ---- 3: candidates -------------------------------------------------------------------------------------------

def t3_candidates(f):
    ic = context(f)
    typ(ic, "g4")  # ㄕˋ: more than two pages of candidates
    first = text(ic)
    h, _ = ic.tap(DOWN)
    assert h and ic.cands, "Down opens the list"
    p1 = ic.cands
    assert [l for l, _ in p1["items"]] == [f"{i}. " for i in range(1, 10)], p1
    assert p1["index"] == 0 and p1["items"][0][1] == first and not p1["has_prev"] and p1["has_next"], p1
    assert text(ic) == first and ic.commits == []
    # The highlight follows the engine.
    ic.tap(DOWN)
    assert ic.cands == dict(p1, index=1), ic.cands
    ic.tap(UP)
    assert ic.cands == p1
    # Paging from the panel (what a click on its arrows does) and by the keys gives the same pages.
    ic.page(True)
    p2 = ic.cands
    assert p2 and p2["has_prev"] and p2["index"] == 0 and len(p2["items"]) == 9, p2
    assert not {t for _, t in p1["items"]} & {t for _, t in p2["items"]}, "the second page has other candidates"
    ic.page(False)
    assert ic.cands == p1
    h, _ = ic.tap(PAGE_DOWN)
    assert h and ic.cands == p2
    ic.page(True)
    p3 = ic.cands
    assert p3["has_prev"] and p3["items"] != p2["items"]
    h, _ = ic.tap(PAGE_UP)
    assert h and ic.cands == p2
    # A digit picks from the page shown.
    h, _ = ic.tap(ord("3"))
    assert h and ic.cands is None and text(ic) == p2["items"][2][1] and ic.commits == [], (ic.cands, ic.preedit)
    # A click on a candidate.
    ic.tap(DOWN)
    page = ic.cands
    ic.select(4)
    assert ic.cands is None and text(ic) == page["items"][4][1] and ic.commits == [], (ic.cands, ic.preedit)
    picked = text(ic)
    # Esc closes the list and nothing else.
    ic.tap(DOWN)
    assert ic.cands
    h, _ = ic.tap(ESC)
    assert h and ic.cands is None and text(ic) == picked
    ic.tap(RETURN)
    assert ic.commits == [picked] and ic.preedit == ("", 0)
    # Word candidates of a longer composition, and a list of one page.
    typ(ic, "su3cl3")
    ic.tap(DOWN)
    assert ic.cands and ic.cands["items"][0] == ("1. ", "你好") and not ic.cands["has_next"] and not ic.cands["has_prev"], ic.cands
    ic.select(1)
    second = text(ic)
    ic.tap(RETURN)
    assert ic.commits == [picked, second] and second != "你好"


# ---- 4: Shift tap, status display ------------------------------------------------------------------------------

def shift_tap(ic, sym):
    h1, _ = ic.key(sym)
    h2, _ = ic.key(sym, SHIFT, release=True)
    assert not h1 and not h2, "a Shift press and release always go to the application"


def wait_status(f, kim, label):
    end = time.monotonic() + 2.0
    while f"label={label}" not in kim.status():
        assert time.monotonic() < end, f"status display does not show {label}: {kim.status()!r}"
        f.fx.pump(0.02)


def t4_shift_tap_and_status(f, kim):
    ic = context(f)
    wait_status(f, kim, "中")
    shift_tap(ic, SHIFT_L)
    wait_status(f, kim, "英")
    assert "英文" in kim.status()
    passes(ic)
    shift_tap(ic, SHIFT_R)
    wait_status(f, kim, "中")
    typ(ic, "su3")
    assert text(ic) == "你"
    ic.tap(RETURN)
    # Not a tap: Shift held with a letter, and Shift held long.
    n = len(kim.props)
    ic.key(SHIFT_L)
    ic.key(ord("A"), SHIFT)
    ic.key(ord("A"), SHIFT, release=True)
    ic.key(SHIFT_L, SHIFT, release=True)
    ic.tap(ESC)
    ic.key(SHIFT_R)
    time.sleep(0.6)
    ic.key(SHIFT_R, SHIFT, release=True)
    f.fx.pump(0.2)
    assert all("label=中" in p for p in kim.props[n:]), kim.props[n:]
    typ(ic, "cl3")
    assert text(ic) == "好"
    ic.tap(RETURN)
    assert ic.commits == ["你", "好"], ic.commits


t4_shift_tap_and_status.kim = True

def t4_left_shift_with_stock_fcitx5_hotkeys(f):
    """Fcitx5 as installed takes a left Shift tap for itself (its 'temporarily switch to the first input method' key).
    Recorded here: the two do not fight - the tap switches to the keyboard layout and back, our mode stays Chinese."""
    ic = context(f)
    shift_tap(ic, SHIFT_L)
    f.fx.pump(0.1)
    im = f.fx.controller("CurrentInputMethod").unpack()[0]
    note(f"stock hotkeys: after a left Shift tap the input method of the context is {im!r}")
    h, _ = ic.tap(ord("s"))
    assert not h and ic.preedit == ("", 0)
    shift_tap(ic, SHIFT_L)
    f.fx.pump(0.1)
    assert f.fx.controller("CurrentInputMethod").unpack()[0] == "twpotato"
    typ(ic, "su3")
    assert text(ic) == "你"
    # The right Shift is ours.
    ic.tap(RETURN)
    shift_tap(ic, SHIFT_R)
    passes(ic)
    shift_tap(ic, SHIFT_R)
    typ(ic, "cl3")
    assert text(ic) == "好" and ic.commits == ["你"]


t4_left_shift_with_stock_fcitx5_hotkeys.stock_hotkeys = True

# ---- 5: password and sensitive fields --------------------------------------------------------------------------
# Password: Fcitx5 itself puts the keyboard layout in place of the input method. Sensitive: the addon gets the keys
# and the shell passes them. Both are told to the engine as TWIME_EV_SENSITIVE.
FIELDS = (("password", CAP_PASSWORD), ("sensitive", CAP_SENSITIVE))


def t5_field_marked_from_the_start(f):
    context(f).focus_out()  # the engine is up
    for name, cap in FIELDS:
        ic = f.fx.context(CAP_DEFAULT | cap)
        ic.focus_in()
        passes(ic)
        assert ic.commits == [], name
        ic.focus_out()
        ic.destroy()
    assert engine_status()[1] == 0


def t5_ordinary_to_sensitive_and_back_without_a_focus_change(f):
    for name, cap in FIELDS:
        ic = context(f)
        typ(ic, "su3")
        assert text(ic) == "你"
        ic.set_caps(CAP_DEFAULT | cap)
        f.fx.pump(0.05)
        assert ic.preedit == ("", 0) and ic.cands is None, f"{name}: the composition ends at the change: {ic.preedit}"
        assert ic.commits == [] and engine_status()[1] == 0, (name, ic.commits)
        passes(ic)
        ic.set_caps(CAP_DEFAULT)
        f.fx.pump(0.05)
        typ(ic, "cl3")
        assert text(ic) == "好", name
        ic.tap(RETURN)
        assert ic.commits == ["好"], (name, ic.commits)
        ic.focus_out()
        ic.destroy()


# (text committed before, word typed after): without the earlier text the word converts differently.
CONTEXT_PAIRS = (("ㄌㄢˊ ㄑㄧㄡˊ ㄅㄧˇ ㄙㄞˋ", "ㄍㄨㄥ ㄕˋ"), ("ㄧㄣ ㄩㄝˋ ㄏㄨㄟˋ", "ㄒㄧㄥˊ ㄕˋ"), ("ㄧ ㄕㄥ ㄕㄨㄛ", "ㄧㄡˊ ㄩˊ"),
                 ("ㄉㄧㄢˋ ㄋㄠˇ ㄔㄥˊ ㄕˋ", "ㄏㄨㄟˋ ㄏㄨㄚˋ"))


def t5_sensitive_field_clears_the_earlier_text(f):
    """After a visit to a sensitive state the engine must not remember what was typed before. Control: the same word
    converts differently when the earlier text is remembered, so a cleared memory shows."""
    ic = context(f)

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
        ic.focus_out(); ic.focus_in()
        alone = word(after)
        ic.focus_out(); ic.focus_in()
        sentence(before)
        remembered = word(after)
        if remembered == alone:
            note(f"context pair without effect on this model (not used): {ic.commits[-1]} + {alone}")
            continue
        used += 1
        for name, cap in FIELDS:
            ic.focus_out(); ic.focus_in()
            sentence(before)
            ic.set_caps(CAP_DEFAULT | cap)
            f.fx.pump(0.05)
            passes(ic)
            ic.set_caps(CAP_DEFAULT)
            f.fx.pump(0.05)
            cleared = word(after)
            assert cleared == alone, f"{name}: after {ic.commits[-1]} the word is {cleared}, with empty memory {alone}, with memory {remembered}"
        note(f"context control: after {ic.commits[-1]!r} the word is {remembered!r}; alone and after a password / sensitive state {alone!r}")
    assert used >= 2, "the control pairs no longer depend on the earlier text: choose new ones"


def t5_reset_without_a_composition_clears_the_earlier_text(f):
    """A reset from the application (the caret went elsewhere) while nothing is being composed: the engine forgets what
    was typed before. Same control as above."""
    ic = context(f)

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
        ic.focus_out(); ic.focus_in()
        alone = word(after)
        ic.focus_out(); ic.focus_in()
        sentence(before)
        remembered = word(after)
        if remembered == alone:
            continue
        used += 1
        ic.focus_out(); ic.focus_in()
        sentence(before)
        assert ic.preedit == ("", 0)
        ic.reset()
        cleared = word(after)
        assert cleared == alone, f"after {ic.commits[-1]} and a reset the word is {cleared}, with empty memory {alone}, with memory {remembered}"
    assert used >= 2, "the control pairs no longer depend on the earlier text: choose new ones"
    note(f"reset without a composition: the earlier text is forgotten ({used} control pairs)")


def t5_engine_ends_while_a_sensitive_field_has_the_focus(f):
    ic = context(f)
    typ(ic, "su3")
    ic.set_caps(CAP_DEFAULT | CAP_SENSITIVE)
    f.fx.pump(0.05)
    passes(ic)
    first = engines()
    assert len(first) == 1
    kill_engines()
    t = time.monotonic()
    passes(ic)  # the first of these keys finds the connection dead and starts another engine
    while True:
        passes(ic, "s")
        try:
            if engines() and engines() != first and engine_status()[0] == 1:
                break
        except OSError:
            pass
        assert time.monotonic() - t < 10, "the engine did not come back"
        time.sleep(0.05)
    took = time.monotonic() - t
    assert took < 10, f"the engine came back after {took:.2f} s"
    note(f"sensitive field, engine ended: a new engine and session after {took:.2f} s")
    passes(ic)
    assert engine_status()[:2] == (1, 0), "a new session exists and nothing was composed in it"
    assert ic.commits == []
    ic.set_caps(CAP_DEFAULT)
    f.fx.pump(0.05)
    typ(ic, "su3")
    assert text(ic) == "你" and engine_status()[:2] == (1, 1)


def t5_engine_ends_while_a_password_field_has_the_focus(f):
    ic = context(f)
    ic.set_caps(CAP_DEFAULT | CAP_PASSWORD)
    f.fx.pump(0.05)
    passes(ic)
    kill_engines()
    passes(ic)
    time.sleep(1.5)
    passes(ic)
    assert ic.commits == []
    ic.set_caps(CAP_DEFAULT)
    f.fx.pump(0.05)
    took = ready(ic, 3.0)
    typ(ic, "su3")
    assert text(ic) == "你", ic.preedit
    note(f"password field, engine ended: input possible {took:.2f} s after the field became ordinary again")


# ---- 6: two input contexts -------------------------------------------------------------------------------------

def t6_two_contexts(f):
    a = context(f)
    b = f.fx.context()
    typ(a, "su3")
    assert text(a) == "你"
    # Leaving a context: its composition is handled as the engine's reply to the focus loss says (it commits it).
    a.focus_out()
    f.fx.pump(0.05)
    assert a.preedit == ("", 0) and a.commits == ["你"], (a.preedit, a.commits)
    b.focus_in()
    typ(b, "cl3")
    assert engine_status()[:2] == (2, 1)
    assert text(b) == "好" and b.commits == []
    b.tap(RETURN)
    assert b.commits == ["好"]
    b.focus_out()
    a.focus_in()
    typ(a, "cl3")
    a.tap(RETURN)
    assert a.commits == ["你", "好"] and b.commits == ["好"], (a.commits, b.commits)
    # Syllable by syllable in turns: every focus loss ends the composition of that moment in its own context.
    for ka, kb in (("su3", "cl3"), ("cl3", "su3")):
        a.focus_in(); typ(a, ka); a.focus_out()
        b.focus_in(); typ(b, kb); b.focus_out()
    f.fx.pump(0.05)
    assert a.commits[2:] == ["你", "好"] and b.commits[1:] == ["好", "你"], (a.commits, b.commits)
    # Both composing at the same moment (two windows, no focus change reported in between).
    a.focus_in(); b.focus_in()
    typ(a, "su3"); typ(b, "cl3"); typ(a, "cl3"); typ(b, "su3")
    assert text(a) == "你好" and text(b) == "好你", (a.preedit, b.preedit)
    assert engine_status()[:2] == (2, 2)
    a.tap(RETURN); b.tap(RETURN)
    assert a.commits[-1] == "你好" and b.commits[-1] == "好你"
    # A session ends with its input context.
    a.focus_out(); a.destroy()
    time.sleep(0.1)
    assert engine_status() == (1, 0, 0), engine_status()
    b.focus_out(); b.destroy()
    time.sleep(0.1)
    assert engine_status() == (0, 0, 0), engine_status()


# ---- 7: faults -------------------------------------------------------------------------------------------------

def held_typing(ic, keys, forwarded_of, wait_for, what):
    """Types `keys` at once (each press and release must be held: reported handled within the time limit), then waits
    for `wait_for()`. Returns (slowest key in ms, seconds until wait_for held)."""
    t = time.monotonic()
    slowest = 0.0
    for k in keys:
        for release in (False, True):
            h, dt = ic.key(ord(k), release=release)
            slowest = max(slowest, dt * 1000)
            assert h, f"{what}: key {k!r} {'release' if release else 'press'} was not held"
    end = time.monotonic() + 3.5
    while not wait_for():
        assert time.monotonic() < end, f"{what}: commits {ic.commits} forwarded {forwarded_of(ic)}"
        pump(ic, 0.01)
    return slowest, time.monotonic() - t


def pump(ic, s):
    ic.fx.pump(s)


def t7i_keys_typed_while_the_engine_starts_wait_for_it(f):
    """The engine is started at the focus; keys typed before it answers are held and reach it in order (LINUX-plan 7.7)."""
    assert not engines() and not starts()
    ic = f.fx.context()
    ic.focus_in()
    end = time.monotonic() + 1
    while not starts():
        assert time.monotonic() < end, "the engine is started at the focus, not at the first key"
        time.sleep(0.01)
    slowest, took = held_typing(ic, "su3cl3", lambda c: c.forwarded, lambda: ic.preedit[0] == "你好", "cold start")
    assert slowest < CALL_TIMEOUT_MS + 20, f"a held key took {slowest:.1f} ms"
    # The presses were the engine's; the releases go back to the application, in order.
    assert [(sym, rel) for sym, _st, rel in ic.forwarded] == [(ord(k), True) for k in "su3cl3"], ic.forwarded
    ic.tap(RETURN)
    assert ic.commits == ["你好"] and len(starts()) == 1 and len(engines()) == 1
    note(f"cold start: typed at once after the focus; slowest key {slowest:.1f} ms; the composition was there after {took:.2f} s")


def t7vi_held_keys_go_back_unchanged_when_the_engine_does_not_come(f):
    """The engine is started at the focus and stopped before it is ready: after 2 s the held keys go to the application
    unchanged and in order, and the next keys are not held again."""
    ic = f.fx.context()
    ic.focus_in()
    end = time.monotonic() + 1
    while not engines():
        assert time.monotonic() < end
        time.sleep(0.005)
    (pid,) = engines()
    os.kill(pid, signal.SIGSTOP)
    try:
        slowest, took = held_typing(ic, "su3", lambda c: c.forwarded, lambda: len(ic.forwarded) == 6, "engine stopped while loading")
        assert 1.9 < took < 2.5, took
        assert [(sym, rel) for sym, _st, rel in ic.forwarded] == [(ord(k), r) for k in "su3" for r in (False, True)], ic.forwarded
        h, dt = ic.tap(ord("s"))
        assert not h and dt * 1000 < CALL_TIMEOUT_MS + 20, "after a hold that ran out, keys pass at once"
    finally:
        os.kill(pid, signal.SIGCONT)
    ready(ic, 3.0)
    typ(ic, "su3cl3")
    ic.tap(RETURN)
    assert ic.commits == ["你好"], ic.commits
    note(f"engine stopped while loading: held keys went on unchanged after {took:.2f} s")


def t7vii_another_event_sends_held_keys_on_first(f):
    """A focus loss while keys are held: they go on to the application unchanged, before it."""
    ic = f.fx.context()
    ic.focus_in()
    end = time.monotonic() + 1
    while not engines():
        assert time.monotonic() < end
        time.sleep(0.005)
    (pid,) = engines()
    os.kill(pid, signal.SIGSTOP)
    try:
        assert ic.tap(ord("s"))[0]
        ic.focus_out()
        f.fx.pump(0.05)
        assert [(sym, rel) for sym, _st, rel in ic.forwarded] == [(ord("s"), False), (ord("s"), True)], ic.forwarded
    finally:
        os.kill(pid, signal.SIGCONT)
    assert ic.commits == []


def t7ii_engine_ends_while_composing(f):
    ic = context(f)
    typ(ic, "su3")
    assert text(ic) == "你"
    kill_engines()
    t = time.monotonic()
    # The key finds the engine gone: the composition is lost, a new engine is started, and the key waits for it.
    h, dt = ic.key(ord("c"))
    assert h, "the key is held for the new engine"
    assert dt * 1000 < CALL_TIMEOUT_MS + 20, f"the key took {dt * 1000:.1f} ms"
    assert ic.preedit == ("", 0) and ic.cands is None, "the composition on screen is removed"
    ic.key(ord("c"), release=True)
    end = time.monotonic() + 3
    while text(ic) != "ㄏ":
        assert time.monotonic() < end, "the held key did not reach the new engine"
        f.fx.pump(0.01)
    took = time.monotonic() - t
    typ(ic, "l3")
    assert text(ic) == "好"
    ic.tap(RETURN)
    assert ic.commits == ["好"], ic.commits
    assert len(starts()) == 2
    note(f"engine ended while composing: the key returned after {dt * 1000:.1f} ms; it reached the new engine after {took:.2f} s")


def t7iii_engine_does_not_answer(f):
    """The engine is stopped (SIGSTOP) in the middle of a composition: the call runs into the shell's time limit.
    (The engine's own delay injection needs a session id the C interface cannot choose; see twime-shell's
    tests/engine.rs, which covers that variant.)"""
    ic = context(f)
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
    assert not h and ic.preedit == ("", 0) and ic.cands is None and ic.commits == []
    assert CALL_TIMEOUT_MS <= ms < CALL_TIMEOUT_MS + 20, f"the key returned after {ms:.1f} ms"
    ic.key(ord("c"), release=True)
    typ(ic, "cl3")
    assert text(ic) == "好"
    ic.tap(RETURN)
    assert ic.commits == ["好"] and len(starts()) == 1
    note(f"engine stopped: the key returned after {ms:.1f} ms (limit {CALL_TIMEOUT_MS} + 20)")


def t7iv_assets_missing(f):
    target = os.readlink(ASSETS_LINK)
    os.unlink(ASSETS_LINK)
    try:
        ic = f.fx.context()
        t = time.monotonic()
        ic.focus_in()  # the first start
        seen, slowest, taps, passed = [], 0.0, 0, 0
        while time.monotonic() - t < 7.5:
            h, dt = ic.tap(ord("s"))
            taps += 1
            passed += not h  # a key that comes while a start is still under way is held for that moment
            slowest = max(slowest, dt)
            assert ic.preedit == ("", 0)
            if len(starts()) > len(seen):
                seen.append(time.monotonic() - t)
            f.fx.pump(0.05)
    finally:
        os.symlink(target, ASSETS_LINK)
    time.sleep(0.5)
    f.fx.pump(0.1)
    # Every key reached the application once, in order: directly, or held and then sent on unchanged.
    assert passed + sum(1 for _s, _st, rel in ic.forwarded if not rel) == taps, (passed, ic.forwarded, taps)
    at = starts()
    assert len(at) == 3, f"engine starts at {seen}"
    # The shell waits 3 s between two starts; the wrapper notes its own start time a moment later (tolerance 50 ms).
    assert all(b - a >= 2.95 for a, b in zip(at, at[1:])), at
    assert not engines() and ic.commits == []
    # The engines that gave up were waited for: no zombie is left under fcitx5.
    for d in os.listdir("/proc"):
        if d.isdigit():
            try:
                s = open(f"/proc/{d}/stat").read()
            except OSError:
                continue
            assert not ("(twime-engined)" in s and s[s.rindex(")") + 2] == "Z"), s
    note(f"assets missing: 3 engine starts in 7.5 s ({', '.join(f'{x:.2f}' for x in seen)} s); slowest key {slowest * 1000:.1f} ms")
