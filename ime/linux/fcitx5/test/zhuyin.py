"""Bopomofo syllables -> the keys that type them on the standard layout, as ime/xcmp does (twime-core fuzzy::keys_of):
the keys of initial, medial and final, then the tone key (first tone: Space; the neutral tone key comes last too)."""

KEY_OF = dict(zip("ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙㄧㄨㄩㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦ",
                  "1qaz2wsxedcrfv5tgbyhnujm8ik,9ol.0p;/-"))
TONE_KEY = {"ˊ": "6", "ˇ": "3", "ˋ": "4", "˙": "7"}


def keys_of(syllable):
    """The characters of the keys of one syllable (a space for the Space key)."""
    body = [c for c in syllable if c not in TONE_KEY]
    tone = [c for c in syllable if c in TONE_KEY]
    assert body and len(tone) <= 1 and all(c in KEY_OF for c in body), syllable
    return "".join(KEY_OF[c] for c in body) + (TONE_KEY[tone[0]] if tone else " ")


def keys_of_line(zhuyin):
    """Syllables separated by spaces -> one string of key characters per syllable."""
    return [keys_of(s) for s in zhuyin.split(" ") if s]
