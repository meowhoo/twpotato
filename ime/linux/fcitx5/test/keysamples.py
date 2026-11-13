"""Key symbols for the test "which keys are placed by their key code", shared by the Fcitx5 and the IBus tests so
that both shells are held to the same list. Each is sent together with the key code of the physical S key.

(name, key symbol, types text): a text key is placed by the code and types ㄋ (the bopomofo of the S key); any other
key keeps the meaning of its symbol and must not.
OWN: what such a key commits by its own meaning (the number pad digit); the others commit nothing."""

OWN = {"KP_1": ["1"]}

SAMPLES = (
    ("a", 0x61, True), ("Z", 0x5a, True), ("7", 0x37, True), ("semicolon", 0x3b, True), ("asciitilde", 0x7e, True),
    ("eacute (Latin-1)", 0xe9, True), ("U+00E9 as a direct Unicode symbol", 0x010000e9, True),
    ("Cyrillic_yeru", 0x6d9, True), ("Greek_sigma", 0x7f2, True), ("dead_acute", 0xfe51, True),
    ("Tab", 0xff09, False), ("BackSpace", 0xff08, False), ("Delete", 0xffff, False), ("Escape", 0xff1b, False),
    ("space", 0x20, False), ("KP_1", 0xffb1, False), ("ISO_Left_Tab", 0xfe20, False),
    ("3270_BackTab", 0xfd05, False), ("AccessX_Enable", 0xfe70, False), ("Terminate_Server", 0xfed5, False),
    ("Pointer_Left", 0xfee0, False), ("a surrogate as a direct Unicode symbol", 0x0100d800, False),
    ("U+0009 as a direct Unicode symbol", 0x01000009, False), ("XF86AudioPlay", 0x1008ff14, False),
    ("F1", 0xffbe, False), ("nobreakspace", 0xa0, False),
) + tuple(
    # The symbols for which IBus's and Fcitx5's conversions to a character disagree (see listed_text / listedText in the
    # shells; `blank` 0x9df among them): all of them type text.
    (f"listed {v:#x}", v, True) for v in (0x8a1, 0x8a2, 0x8a3, *range(0x8a7, 0x8b1), 0x8c9, 0x9df, 0x9ef, 0x9f0, 0x9f2, 0x9f3,
                                          0xaac, 0xaaf, 0xad5, 0xef3, 0xef9, 0x13a4)
) + (
    ("ch (Czech)", 0xfea0, True),
    ("Shift_L", 0xffe1, False),  # last: its tap switches the mode
)
