// TwPotato for Fcitx5: translates Fcitx5 events into twime_ctx_event calls and each reply into Fcitx5's commit /
// preedit / candidate calls. No input method logic lives here: all of it is in libtwime_shell.a (twime_shell.h) and
// in the engine behind it.
//
// Written against the public Fcitx5 interface (the installed headers) only.
#include <cstdint>
#include <ctime>
#include <deque>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/handlertable.h>
#include <fcitx-utils/key.h>
#include <fcitx-utils/keysym.h>
#include <fcitx-utils/textformatflags.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/candidatelist.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputcontextproperty.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/inputmethodentry.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>

#include "twime_shell.h"

namespace {

using namespace fcitx;

// Fcitx5 key symbol -> TWIME_KEY_*: the key at that place of a US keyboard, whatever Shift or Caps Lock made of it.
// (Fcitx5 hands an input method the symbols of the layout the input method is configured with.)
uint32_t keyOf(KeySym sym) {
    if (sym >= FcitxKey_a && sym <= FcitxKey_z) return sym;
    if (sym >= FcitxKey_A && sym <= FcitxKey_Z) return sym - FcitxKey_A + FcitxKey_a;
    if (sym >= FcitxKey_0 && sym <= FcitxKey_9) return sym;
    if (sym >= FcitxKey_KP_0 && sym <= FcitxKey_KP_9) return TWIME_KEY_NUMPAD_0 + (sym - FcitxKey_KP_0);
    switch (sym) {
    case FcitxKey_parenright: return '0';
    case FcitxKey_exclam: return '1';
    case FcitxKey_at: return '2';
    case FcitxKey_numbersign: return '3';
    case FcitxKey_dollar: return '4';
    case FcitxKey_percent: return '5';
    case FcitxKey_asciicircum: return '6';
    case FcitxKey_ampersand: return '7';
    case FcitxKey_asterisk: return '8';
    case FcitxKey_parenleft: return '9';
    case FcitxKey_minus: case FcitxKey_underscore: return TWIME_KEY_MINUS;
    case FcitxKey_comma: case FcitxKey_less: return TWIME_KEY_COMMA;
    case FcitxKey_period: case FcitxKey_greater: return TWIME_KEY_PERIOD;
    case FcitxKey_slash: case FcitxKey_question: return TWIME_KEY_SLASH;
    case FcitxKey_semicolon: case FcitxKey_colon: return TWIME_KEY_SEMICOLON;
    case FcitxKey_grave: case FcitxKey_asciitilde: return TWIME_KEY_BACKQUOTE;
    case FcitxKey_bracketleft: case FcitxKey_braceleft: return TWIME_KEY_LBRACKET;
    case FcitxKey_bracketright: case FcitxKey_braceright: return TWIME_KEY_RBRACKET;
    case FcitxKey_apostrophe: case FcitxKey_quotedbl: return TWIME_KEY_QUOTE;
    case FcitxKey_backslash: case FcitxKey_bar: return TWIME_KEY_BACKSLASH;
    case FcitxKey_equal: case FcitxKey_plus: return TWIME_KEY_EQUALS;
    case FcitxKey_space: return TWIME_KEY_SPACE;
    case FcitxKey_Return: case FcitxKey_KP_Enter: return TWIME_KEY_ENTER;
    case FcitxKey_Escape: return TWIME_KEY_ESC;
    case FcitxKey_BackSpace: return TWIME_KEY_BACKSPACE;
    case FcitxKey_Delete: case FcitxKey_KP_Delete: return TWIME_KEY_DELETE;
    case FcitxKey_Tab: case FcitxKey_ISO_Left_Tab: return TWIME_KEY_TAB;
    case FcitxKey_Left: case FcitxKey_KP_Left: return TWIME_KEY_LEFT;
    case FcitxKey_Right: case FcitxKey_KP_Right: return TWIME_KEY_RIGHT;
    case FcitxKey_Up: case FcitxKey_KP_Up: return TWIME_KEY_UP;
    case FcitxKey_Down: case FcitxKey_KP_Down: return TWIME_KEY_DOWN;
    case FcitxKey_Home: case FcitxKey_KP_Home: return TWIME_KEY_HOME;
    case FcitxKey_End: case FcitxKey_KP_End: return TWIME_KEY_END;
    case FcitxKey_Page_Up: case FcitxKey_KP_Page_Up: return TWIME_KEY_PAGE_UP;
    case FcitxKey_Page_Down: case FcitxKey_KP_Page_Down: return TWIME_KEY_PAGE_DOWN;
    case FcitxKey_KP_Decimal: return TWIME_KEY_NUMPAD_DECIMAL;
    case FcitxKey_KP_Add: return TWIME_KEY_NUMPAD_ADD;
    case FcitxKey_KP_Subtract: return TWIME_KEY_NUMPAD_SUBTRACT;
    case FcitxKey_KP_Multiply: return TWIME_KEY_NUMPAD_MULTIPLY;
    case FcitxKey_KP_Divide: return TWIME_KEY_NUMPAD_DIVIDE;
    case FcitxKey_Shift_L: return TWIME_KEY_SHIFT_LEFT;
    case FcitxKey_Shift_R: return TWIME_KEY_SHIFT_RIGHT;
    case FcitxKey_Control_L: case FcitxKey_Control_R: case FcitxKey_Alt_L: case FcitxKey_Alt_R: case FcitxKey_Meta_L:
    case FcitxKey_Meta_R: case FcitxKey_Super_L: case FcitxKey_Super_R: case FcitxKey_Hyper_L: case FcitxKey_Hyper_R:
    case FcitxKey_ISO_Level3_Shift: case FcitxKey_ISO_Level5_Shift: case FcitxKey_Mode_switch: case FcitxKey_Caps_Lock:
    case FcitxKey_Shift_Lock: case FcitxKey_Num_Lock: return TWIME_KEY_MODIFIER;
    default: return TWIME_KEY_OTHER;
    }
}

// Linux input event code (the numbering of linux/input-event-codes.h) -> TWIME_KEY_*, for the keys whose symbol depends
// on the keyboard layout: the letters, the digit row and the punctuation keys. The bopomofo layout is tied to the
// places of the keys; under a system layout other than US (Dvorak, ...) the symbol of a key says another place.
// TWIME_KEY_OTHER: not one of these keys (Enter, arrows, the number pad, ...): the symbol decides, see keyOf.
// The same table is in ../../ibus/src/main.c (key_at): keep the two alike.
uint32_t keyAt(int code) {
    static const char digits[] = "1234567890"; // KEY_1 (2) .. KEY_0 (11)
    static const char top[] = "qwertyuiop";    // KEY_Q (16) .. KEY_P (25)
    static const char home[] = "asdfghjkl";    // KEY_A (30) .. KEY_L (38)
    static const char bottom[] = "zxcvbnm";    // KEY_Z (44) .. KEY_M (50)
    if (code >= 2 && code <= 11) return static_cast<uint32_t>(digits[code - 2]);
    if (code >= 16 && code <= 25) return static_cast<uint32_t>(top[code - 16]);
    if (code >= 30 && code <= 38) return static_cast<uint32_t>(home[code - 30]);
    if (code >= 44 && code <= 50) return static_cast<uint32_t>(bottom[code - 44]);
    switch (code) {
    case 12: return TWIME_KEY_MINUS;      // KEY_MINUS
    case 13: return TWIME_KEY_EQUALS;     // KEY_EQUAL
    case 26: return TWIME_KEY_LBRACKET;   // KEY_LEFTBRACE
    case 27: return TWIME_KEY_RBRACKET;   // KEY_RIGHTBRACE
    case 39: return TWIME_KEY_SEMICOLON;  // KEY_SEMICOLON
    case 40: return TWIME_KEY_QUOTE;      // KEY_APOSTROPHE
    case 41: return TWIME_KEY_BACKQUOTE;  // KEY_GRAVE
    case 43: return TWIME_KEY_BACKSLASH;  // KEY_BACKSLASH
    case 51: return TWIME_KEY_COMMA;      // KEY_COMMA
    case 52: return TWIME_KEY_PERIOD;     // KEY_DOT
    case 53: return TWIME_KEY_SLASH;      // KEY_SLASH
    default: return TWIME_KEY_OTHER;
    }
}

// A character a text key types: a Unicode scalar value that is not a control character and not a blank.
// (The same list as text_char in ../../ibus/src/main.c: keep the two alike.)
bool textChar(uint32_t u) {
    if (u < 0x21 || (u >= 0x7f && u <= 0xa0) || (u >= 0xd800 && u <= 0xdfff) || u > 0x10ffff) return false;
    return !(u == 0x1680 || (u >= 0x2000 && u <= 0x200a) || u == 0x2028 || u == 0x2029 || u == 0x202f || u == 0x205f || u == 0x3000);
}

// Key symbols that stand for a character, but not in the tables of both frameworks: for these one of IBus's and Fcitx5's
// conversions gives a character and the other none (found by comparing the two over all values up to 0xffff; `blank`
// differs between versions). Technical and box drawing pieces, a few marks, two Hangul letters, the euro sign: all text.
// Named here so that the two shells agree. (The same list as listed_text in ../../ibus/src/main.c.)
bool listedText(uint32_t v) {
    return (v >= 0x8a1 && v <= 0x8a3) || (v >= 0x8a7 && v <= 0x8b0) || v == 0x8c9 || v == 0x9df || v == 0x9ef || v == 0x9f0 ||
           v == 0x9f2 || v == 0x9f3 || v == 0xaac || v == 0xaaf || v == 0xad5 || v == 0xef3 || v == 0xef9 || v == 0x13a4;
}

// A key symbol that types text (a letter, digit or punctuation mark of any layout) - as opposed to the control,
// function, modifier and number pad keys, and Space. Only text keys are placed by their key code: Escape stays Escape
// whatever code comes with it. Decided by the character Fcitx5 says the symbol stands for; the dead keys and the Czech
// "ch" symbols have none and are named by their ranges in the key symbol definitions. (As types_text in the IBus shell.)
bool typesText(KeySym sym) {
    const uint32_t v = sym;
    if (v >= FcitxKey_KP_Space && v <= FcitxKey_KP_Equal) return false; // the number pad: digits, but not text keys
    if ((v >= 0xfe50 && v <= 0xfe6f) || (v >= 0xfe80 && v <= 0xfe8c) || (v >= 0xfe90 && v <= 0xfe93)) return true; // dead_*
    if (v >= 0xfea0 && v <= 0xfea5) return true; // ch, Ch, CH, c_h, C_h, C_H
    if (listedText(v)) return true;
    return textChar(Key::keySymToUnicode(sym));
}

uint32_t modsOf(KeyStates s) {
    uint32_t m = 0;
    if (s.test(KeyState::Shift)) m |= TWIME_MOD_SHIFT;
    if (s.test(KeyState::Ctrl)) m |= TWIME_MOD_CTRL;
    if (s.test(KeyState::Alt)) m |= TWIME_MOD_ALT;
    if (s.testAny(KeyStates{KeyState::Super, KeyState::Super2, KeyState::Hyper, KeyState::Hyper2, KeyState::Meta})) m |= TWIME_MOD_SUPER;
    if (s.test(KeyState::CapsLock)) m |= TWIME_MOD_CAPS;
    return m;
}

// Byte offset of the character with this index (twime_reply.cursor counts characters, Fcitx5 counts UTF-8 bytes).
int byteOffset(twime_str s, uint32_t chars) {
    size_t i = 0;
    for (; i < s.len && chars > 0; chars--) {
        i++;
        while (i < s.len && (static_cast<unsigned char>(s.ptr[i]) & 0xC0) == 0x80) i++;
    }
    return static_cast<int>(i);
}

// What an input context owns: its twime_ctx (made when first needed), the mode last heard, and the key events the
// library holds (TWIME_R_HELD), oldest first.
struct State : public InputContextProperty {
    ~State() override { twime_ctx_free(ctx); }
    twime_ctx *ctx = nullptr;
    uint32_t mode = 0;
    std::deque<std::pair<Key, bool>> held; // the key, is a release
};

class Engine;

class Word : public CandidateWord {
public:
    Word(Engine *engine, uint32_t index, std::string text) : CandidateWord(Text(std::move(text))), engine_(engine), index_(index) {}
    void select(InputContext *ic) const override;

private:
    Engine *engine_;
    uint32_t index_;
};

// The page the engine gave. Paging belongs to the engine: prev / next send it the page keys.
class Page : public CandidateList, public PageableCandidateList {
public:
    Page(Engine *engine, InputContext *ic, const twime_reply &r) : engine_(engine), ic_(ic) {
        for (uint32_t i = 0; i < r.n_candidates; i++) {
            labels_.emplace_back(i < 9 ? std::to_string(i + 1) + ". " : std::string());
            words_.push_back(std::make_unique<Word>(engine, i, std::string(r.candidates[i].ptr, r.candidates[i].len)));
        }
        cursor_ = r.selected < r.n_candidates ? static_cast<int>(r.selected) : -1;
        hasPrev_ = r.page > 0;
        hasNext_ = static_cast<uint64_t>(r.page + 1) * r.per_page < r.total;
        setPageable(this);
    }
    const Text &label(int idx) const override { return labels_.at(idx); }
    const CandidateWord &candidate(int idx) const override { return *words_.at(idx); }
    int size() const override { return static_cast<int>(words_.size()); }
    int cursorIndex() const override { return cursor_; }
    CandidateLayoutHint layoutHint() const override { return CandidateLayoutHint::NotSet; }
    bool hasPrev() const override { return hasPrev_; }
    bool hasNext() const override { return hasNext_; }
    void prev() override;
    void next() override;
    bool usedNextBefore() const override { return true; }

private:
    Engine *engine_;
    InputContext *ic_;
    std::vector<Text> labels_;
    std::vector<std::unique_ptr<Word>> words_;
    int cursor_ = -1;
    bool hasPrev_ = false, hasNext_ = false;
};

class Engine : public InputMethodEngineV2 {
public:
    explicit Engine(Instance *instance) : instance_(instance), factory_([](InputContext &) { return new State; }) {
        twime_shell_open(&shell_);
        instance_->inputContextManager().registerProperty("twpotatoState", &factory_);
        // Password / sensitive may change while the context keeps the focus.
        capabilityWatcher_ = instance_->watchEvent(EventType::InputContextCapabilityChanged, EventWatcherPhase::PostInputMethod, [this](Event &event) {
            auto *ic = static_cast<CapabilityChangedEvent &>(event).inputContext();
            if (ic->propertyFor(&factory_)->ctx) sensitive(ic, instance_->inputMethodEngine(ic) == this);
        });
    }
    ~Engine() override {
        releaseTimer_.reset();
        capabilityWatcher_.reset();
        factory_.unregister(); // frees every State, and with it every twime_ctx
        twime_shell_close(shell_);
    }

    void keyEvent(const InputMethodEntry &, KeyEvent &event) override {
        auto *ic = event.inputContext();
        sensitive(ic, true);
        const Key key = event.rawKey();
        // Fcitx5's key code is the X one: the Linux input event code + 8 (0: the client sent none).
        uint32_t k = key.code() > 8 && typesText(key.sym()) ? keyAt(key.code() - 8) : TWIME_KEY_OTHER;
        if (k == TWIME_KEY_OTHER) k = keyOf(key.sym());
        const twime_reply *r = send(ic, event.isRelease() ? TWIME_EV_KEY_UP : TWIME_EV_KEY_DOWN, k, modsOf(key.states()), false, true);
        if (r && (r->flags & TWIME_R_HELD)) ic->propertyFor(&factory_)->held.emplace_back(key, event.isRelease());
        if (r && (r->flags & TWIME_R_HANDLED)) event.filterAndAccept();
    }
    // Focus in, or the user switched to this input method. Fcitx5 emptied the panel when the context was left:
    // whatever the reply holds is drawn again.
    void activate(const InputMethodEntry &, InputContextEvent &event) override {
        sensitive(event.inputContext(), true);
        send(event.inputContext(), TWIME_EV_FOCUS_IN, 0, 0, true, true);
    }
    // Focus out, or the user switched to another input method: the same to the engine, which commits a composition
    // that can be committed and drops any other. One exception: Fcitx5 switches away by itself because the
    // capabilities of the context changed (reason CapabilityChanged: mainly a field that turned into a password field).
    // Nothing may be committed into such a field: the composition is dropped.
    void deactivate(const InputMethodEntry &, InputContextEvent &event) override {
        const bool capabilityChanged = event.type() == EventType::InputContextSwitchInputMethod &&
                              static_cast<InputContextSwitchInputMethodEvent &>(event).reason() == InputMethodSwitchedReason::CapabilityChanged;
        send(event.inputContext(), capabilityChanged ? TWIME_EV_RESET : TWIME_EV_FOCUS_OUT, 0, 0, true, true);
    }
    void reset(const InputMethodEntry &, InputContextEvent &event) override { send(event.inputContext(), TWIME_EV_RESET, 0, 0, true, true); }

    std::string subMode(const InputMethodEntry &, InputContext &ic) override { return ic.propertyFor(&factory_)->mode ? "英文" : "中文"; }
    std::string subModeLabelImpl(const InputMethodEntry &, InputContext &ic) override { return ic.propertyFor(&factory_)->mode ? "英" : "中"; }

    void pick(InputContext *ic, uint32_t index) { send(ic, TWIME_EV_PICK, index, 0, false, true); }
    void pageKey(InputContext *ic, uint32_t key) { send(ic, TWIME_EV_PANEL_KEY, key, 0, false, true); }

private:
    void sensitive(InputContext *ic, bool show) {
        send(ic, TWIME_EV_SENSITIVE, ic->capabilityFlags().testAny(CapabilityFlag::PasswordOrSensitive) ? 1 : 0, 0, false, show);
    }

    // One event and its reply, applied in the order twime_shell.h gives. `redraw`: show the reply's preedit and
    // candidates whether or not they changed. `show`: false when another input method owns the panel now.
    const twime_reply *send(InputContext *ic, uint32_t kind, uint32_t a, uint32_t b, bool redraw, bool show) {
        auto *state = ic->propertyFor(&factory_);
        if (!state->ctx) {
            if (twime_ctx_new(shell_, &state->ctx) != TWIME_OK) return nullptr;
            // Fcitx5 sends forwarded keys on to the client in order with commits; it commits nothing by itself on a
            // focus loss (DontCommit).
            twime_ctx_event(state->ctx, TWIME_EV_CLIENT, TWIME_CLIENT_DEFER, 0, nullptr);
        }
        const twime_reply *r = nullptr;
        if (twime_ctx_event(state->ctx, kind, a, b, &r) != TWIME_OK || !r) {
            if (show) draw(ic, nullptr);
            return nullptr;
        }
        for (uint32_t i = 0; i < r->release_before; i++) forwardHeld(ic, state);
        if (r->commit.len > 0) ic->commitString(std::string(r->commit.ptr, r->commit.len));
        if (show && (redraw || (r->flags & TWIME_R_UI_CHANGED))) draw(ic, r);
        state->mode = r->mode;
        if (r->flags & TWIME_R_MODE_CHANGED) ic->updateUserInterface(UserInterfaceComponent::StatusArea);
        if (r->flags & TWIME_R_RELEASED) {
            if (r->flags & TWIME_R_HANDLED) {
                if (!state->held.empty()) state->held.pop_front();
            } else {
                forwardHeld(ic, state);
            }
        }
        if (r->flags & TWIME_R_HOLDING) armReleaseTimer();
        return r;
    }

    // Sends the oldest held key event on to the application, unchanged.
    void forwardHeld(InputContext *ic, State *state) {
        if (state->held.empty()) return;
        auto [key, release] = state->held.front();
        state->held.pop_front();
        ic->forwardKey(key, release);
    }

    // While keys are held anywhere: every 10 ms, release what can be released.
    void armReleaseTimer() {
        constexpr uint64_t interval = 10000; // microseconds
        if (!releaseTimer_) {
            releaseTimer_ = instance_->eventLoop().addTimeEvent(CLOCK_MONOTONIC, now(CLOCK_MONOTONIC) + interval, 0,
                                                                [this](EventSourceTime *source, uint64_t) {
                                                                    if (releaseHeld()) {
                                                                        source->setNextInterval(interval);
                                                                        source->setOneShot();
                                                                    }
                                                                    return true;
                                                                });
        } else if (!releaseTimer_->isEnabled()) {
            releaseTimer_->setNextInterval(interval);
            releaseTimer_->setOneShot();
        }
    }

    // True: keys are still held.
    bool releaseHeld() {
        bool holding = false;
        instance_->inputContextManager().foreach([this, &holding](InputContext *ic) {
            auto *state = ic->propertyFor(&factory_);
            if (state->held.empty() || !state->ctx) return true;
            const twime_reply *r;
            do {
                r = send(ic, TWIME_EV_RELEASE, 0, 0, false, instance_->inputMethodEngine(ic) == this);
            } while (r && (r->flags & TWIME_R_RELEASED));
            if (r && (r->flags & TWIME_R_HOLDING)) {
                holding = true;
            } else {
                while (!state->held.empty()) forwardHeld(ic, state); // the library failed: the rest goes on, in order
            }
            return true;
        });
        return holding;
    }

    // Replaces what the context shows by the reply's preedit and candidate page; nullptr: by nothing.
    void draw(InputContext *ic, const twime_reply *r) {
        auto &panel = ic->inputPanel();
        panel.reset();
        if (r && r->preedit.len > 0) {
            // DontCommit: when the context loses the focus, what happens to the composition is the engine's reply to
            // TWIME_EV_FOCUS_OUT, not a commit by Fcitx5.
            Text text(std::string(r->preedit.ptr, r->preedit.len), TextFormatFlags{TextFormatFlag::Underline, TextFormatFlag::DontCommit});
            text.setCursor(byteOffset(r->preedit, r->cursor));
            if (ic->capabilityFlags().test(CapabilityFlag::Preedit)) {
                panel.setClientPreedit(text);
            } else {
                panel.setPreedit(text);
            }
        }
        if (r && (r->flags & TWIME_R_CANDIDATES_OPEN)) panel.setCandidateList(std::make_unique<Page>(this, ic, *r));
        ic->updatePreedit();
        ic->updateUserInterface(UserInterfaceComponent::InputPanel);
    }

    Instance *instance_;
    twime_shell *shell_ = nullptr;
    FactoryFor<State> factory_;
    std::unique_ptr<HandlerTableEntry<EventHandler>> capabilityWatcher_;
    std::unique_ptr<EventSourceTime> releaseTimer_;
};

// The reply replaces the candidate list these objects belong to: nothing of `this` is used after the call.
void Word::select(InputContext *ic) const { engine_->pick(ic, index_); }
void Page::prev() { engine_->pageKey(ic_, TWIME_KEY_PAGE_UP); }
void Page::next() { engine_->pageKey(ic_, TWIME_KEY_PAGE_DOWN); }

class Factory : public AddonFactory {
    AddonInstance *create(AddonManager *manager) override { return new Engine(manager->instance()); }
};

} // namespace

FCITX_ADDON_FACTORY(Factory)
