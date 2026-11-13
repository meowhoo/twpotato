/* ibus-engine-twpotato: TwPotato for IBus. Translates the callbacks of an IBus engine object into twime_ctx_event
 * calls and each reply into IBus's commit / preedit / lookup table / property calls. No input method logic lives
 * here: all of it is in libtwime_shell.a (twime_shell.h) and in the engine process behind it.
 *
 * Started by ibus-daemon (the <exec> of the component file). One engine object <-> one twime_ctx. The engine objects
 * are made with "has-focus-id", so that every focus-in names the client: what the library does with a composition on
 * a focus loss depends on it (TWIME_EV_CLIENT).
 * Written against the public libibus interface (the installed headers and its API documentation) only. */
#include <string.h>

#include <ibus.h>

#include "twime_shell.h"

#define COMPONENT_NAME "org.freedesktop.IBus.TwPotato"
#define ENGINE_NAME "twpotato"
#define MODE_PROPERTY "InputMode"

typedef struct {
    IBusEngine parent;
    twime_ctx *ctx;
    IBusProperty *mode; /* the Chinese / English indicator */
    uint32_t caps;      /* TWIME_MOD_CAPS as the last key event from the keyboard had it */
    GQueue held;        /* HeldKey: the key events the library holds (TWIME_R_HELD), oldest first */
    guint release_timer; /* asks TWIME_EV_RELEASE while keys are held; 0: none */
    gboolean gnome_shell; /* the client of the current focus is gnome-shell */
    guint client_caps;  /* the client's IBusCapabilite flags */
} TwEngine;

typedef struct {
    guint keyval, keycode, state;
} HeldKey;

typedef struct {
    IBusEngineClass parent;
} TwEngineClass;

static GType tw_engine_get_type(void);
G_DEFINE_TYPE(TwEngine, tw_engine, IBUS_TYPE_ENGINE)

static twime_shell *shell;
static gboolean wayland_session; /* the desktop session is a Wayland one (our environment is the session's) */

/* IBus key value (an X key symbol) -> TWIME_KEY_*: the key at that place of a US keyboard, whatever Shift or Caps
 * Lock made of it. */
static uint32_t key_of(guint v) {
    if (v >= IBUS_KEY_a && v <= IBUS_KEY_z) return v;
    if (v >= IBUS_KEY_A && v <= IBUS_KEY_Z) return v - IBUS_KEY_A + IBUS_KEY_a;
    if (v >= IBUS_KEY_0 && v <= IBUS_KEY_9) return v;
    if (v >= IBUS_KEY_KP_0 && v <= IBUS_KEY_KP_9) return TWIME_KEY_NUMPAD_0 + (v - IBUS_KEY_KP_0);
    switch (v) {
    case IBUS_KEY_parenright: return '0';
    case IBUS_KEY_exclam: return '1';
    case IBUS_KEY_at: return '2';
    case IBUS_KEY_numbersign: return '3';
    case IBUS_KEY_dollar: return '4';
    case IBUS_KEY_percent: return '5';
    case IBUS_KEY_asciicircum: return '6';
    case IBUS_KEY_ampersand: return '7';
    case IBUS_KEY_asterisk: return '8';
    case IBUS_KEY_parenleft: return '9';
    case IBUS_KEY_minus: case IBUS_KEY_underscore: return TWIME_KEY_MINUS;
    case IBUS_KEY_comma: case IBUS_KEY_less: return TWIME_KEY_COMMA;
    case IBUS_KEY_period: case IBUS_KEY_greater: return TWIME_KEY_PERIOD;
    case IBUS_KEY_slash: case IBUS_KEY_question: return TWIME_KEY_SLASH;
    case IBUS_KEY_semicolon: case IBUS_KEY_colon: return TWIME_KEY_SEMICOLON;
    case IBUS_KEY_grave: case IBUS_KEY_asciitilde: return TWIME_KEY_BACKQUOTE;
    case IBUS_KEY_bracketleft: case IBUS_KEY_braceleft: return TWIME_KEY_LBRACKET;
    case IBUS_KEY_bracketright: case IBUS_KEY_braceright: return TWIME_KEY_RBRACKET;
    case IBUS_KEY_apostrophe: case IBUS_KEY_quotedbl: return TWIME_KEY_QUOTE;
    case IBUS_KEY_backslash: case IBUS_KEY_bar: return TWIME_KEY_BACKSLASH;
    case IBUS_KEY_equal: case IBUS_KEY_plus: return TWIME_KEY_EQUALS;
    case IBUS_KEY_space: return TWIME_KEY_SPACE;
    case IBUS_KEY_Return: case IBUS_KEY_KP_Enter: return TWIME_KEY_ENTER;
    case IBUS_KEY_Escape: return TWIME_KEY_ESC;
    case IBUS_KEY_BackSpace: return TWIME_KEY_BACKSPACE;
    case IBUS_KEY_Delete: case IBUS_KEY_KP_Delete: return TWIME_KEY_DELETE;
    case IBUS_KEY_Tab: case IBUS_KEY_ISO_Left_Tab: return TWIME_KEY_TAB;
    case IBUS_KEY_Left: case IBUS_KEY_KP_Left: return TWIME_KEY_LEFT;
    case IBUS_KEY_Right: case IBUS_KEY_KP_Right: return TWIME_KEY_RIGHT;
    case IBUS_KEY_Up: case IBUS_KEY_KP_Up: return TWIME_KEY_UP;
    case IBUS_KEY_Down: case IBUS_KEY_KP_Down: return TWIME_KEY_DOWN;
    case IBUS_KEY_Home: case IBUS_KEY_KP_Home: return TWIME_KEY_HOME;
    case IBUS_KEY_End: case IBUS_KEY_KP_End: return TWIME_KEY_END;
    case IBUS_KEY_Page_Up: case IBUS_KEY_KP_Page_Up: return TWIME_KEY_PAGE_UP;
    case IBUS_KEY_Page_Down: case IBUS_KEY_KP_Page_Down: return TWIME_KEY_PAGE_DOWN;
    case IBUS_KEY_KP_Decimal: return TWIME_KEY_NUMPAD_DECIMAL;
    case IBUS_KEY_KP_Add: return TWIME_KEY_NUMPAD_ADD;
    case IBUS_KEY_KP_Subtract: return TWIME_KEY_NUMPAD_SUBTRACT;
    case IBUS_KEY_KP_Multiply: return TWIME_KEY_NUMPAD_MULTIPLY;
    case IBUS_KEY_KP_Divide: return TWIME_KEY_NUMPAD_DIVIDE;
    case IBUS_KEY_Shift_L: return TWIME_KEY_SHIFT_LEFT;
    case IBUS_KEY_Shift_R: return TWIME_KEY_SHIFT_RIGHT;
    case IBUS_KEY_Control_L: case IBUS_KEY_Control_R: case IBUS_KEY_Alt_L: case IBUS_KEY_Alt_R: case IBUS_KEY_Meta_L:
    case IBUS_KEY_Meta_R: case IBUS_KEY_Super_L: case IBUS_KEY_Super_R: case IBUS_KEY_Hyper_L: case IBUS_KEY_Hyper_R:
    case IBUS_KEY_ISO_Level3_Shift: case IBUS_KEY_ISO_Level5_Shift: case IBUS_KEY_Mode_switch: case IBUS_KEY_Caps_Lock:
    case IBUS_KEY_Shift_Lock: case IBUS_KEY_Num_Lock: return TWIME_KEY_MODIFIER;
    default: return TWIME_KEY_OTHER;
    }
}

static uint32_t mods_of(guint state) {
    uint32_t m = 0;
    if (state & IBUS_SHIFT_MASK) m |= TWIME_MOD_SHIFT;
    if (state & IBUS_CONTROL_MASK) m |= TWIME_MOD_CTRL;
    if (state & IBUS_MOD1_MASK) m |= TWIME_MOD_ALT;
    if (state & (IBUS_MOD4_MASK | IBUS_SUPER_MASK | IBUS_HYPER_MASK | IBUS_META_MASK)) m |= TWIME_MOD_SUPER;
    if (state & IBUS_LOCK_MASK) m |= TWIME_MOD_CAPS;
    return m;
}

static IBusText *text_of(twime_str s) {
    gchar *z = g_strndup(s.len ? s.ptr : "", s.len);
    IBusText *t = ibus_text_new_from_string(z);
    g_free(z);
    return t;
}

/* Replaces what the engine object shows by the reply's preedit and candidate page; NULL: by nothing. */
static void draw(TwEngine *e, const twime_reply *r) {
    IBusEngine *engine = IBUS_ENGINE(e);
    if (r && r->preedit.len > 0) {
        IBusText *text = text_of(r->preedit);
        guint chars = 0;
        for (uint32_t i = 0; i < r->n_segments; i++) chars += r->segments[i].chars;
        ibus_text_append_attribute(text, IBUS_ATTR_TYPE_UNDERLINE, IBUS_ATTR_UNDERLINE_SINGLE, 0, (gint)chars);
        /* What becomes of the preedit when the focus leaves or the user switches to another input method is settled
         * here, in advance: IBus lets the client commit it (COMMIT) or drop it (CLEAR), and does not deliver a commit
         * the input method sends at that moment. The engine says which of the two it would do.
         * The caret is in characters on both sides. */
        ibus_engine_update_preedit_text_with_mode(engine, text, r->cursor, TRUE,
                                                  (r->flags & TWIME_R_COMMITTABLE) ? IBUS_ENGINE_PREEDIT_COMMIT : IBUS_ENGINE_PREEDIT_CLEAR);
    } else {
        /* An empty text, not just "hide": nothing may be left for the client to commit later. */
        ibus_engine_update_preedit_text_with_mode(engine, ibus_text_new_from_static_string(""), 0, FALSE, IBUS_ENGINE_PREEDIT_CLEAR);
    }
    if (r && (r->flags & TWIME_R_CANDIDATES_OPEN) && r->n_candidates > 0) {
        /* A table of just the page the engine gave (paging and the highlight belong to the engine). */
        IBusLookupTable *table = ibus_lookup_table_new(r->n_candidates, 0, TRUE, FALSE);
        for (uint32_t i = 0; i < r->n_candidates; i++) {
            gchar label[4];
            ibus_lookup_table_append_candidate(table, text_of(r->candidates[i]));
            g_snprintf(label, sizeof label, "%u", (i + 1) % 10);
            ibus_lookup_table_set_label(table, i, ibus_text_new_from_string(i < 9 ? label : ""));
        }
        if (r->selected < r->n_candidates) ibus_lookup_table_set_cursor_pos(table, r->selected);
        ibus_lookup_table_set_orientation(table, IBUS_ORIENTATION_VERTICAL);
        ibus_engine_update_lookup_table(engine, table, TRUE);
    } else {
        ibus_engine_hide_lookup_table(engine);
    }
}

static void show_mode(TwEngine *e, uint32_t mode) {
    ibus_property_set_label(e->mode, ibus_text_new_from_static_string(mode ? "英文" : "中文"));
    ibus_property_set_symbol(e->mode, ibus_text_new_from_static_string(mode ? "英" : "中"));
    ibus_engine_update_property(IBUS_ENGINE(e), e->mode);
}

/* Sends the oldest held key event on to the application, unchanged. */
static void forward_held(TwEngine *e) {
    HeldKey *k = g_queue_pop_head(&e->held);
    if (!k) return;
    ibus_engine_forward_key_event(IBUS_ENGINE(e), k->keyval, k->keycode, k->state);
    g_free(k);
}

static gboolean release_tick(gpointer data);

/* One event and its reply, applied in the order twime_shell.h gives. `redraw`: show the reply's preedit and
 * candidates whether or not they changed. Returns the reply (NULL: none; nothing of it to apply). */
static const twime_reply *send(TwEngine *e, uint32_t kind, uint32_t a, uint32_t b, gboolean redraw) {
    const twime_reply *r = NULL;
    if (!e->ctx && twime_ctx_new(shell, &e->ctx) != TWIME_OK) return NULL;
    if (twime_ctx_event(e->ctx, kind, a, b, &r) != TWIME_OK || !r) {
        draw(e, NULL);
        return NULL;
    }
    for (uint32_t i = 0; i < r->release_before; i++) forward_held(e);
    /* The text the engine commits on a focus loss is the preedit the client was told to commit (see draw). */
    if (r->commit.len > 0 && kind != TWIME_EV_FOCUS_OUT) ibus_engine_commit_text(IBUS_ENGINE(e), text_of(r->commit));
    if (redraw || (r->flags & TWIME_R_UI_CHANGED)) draw(e, r);
    if (redraw || (r->flags & TWIME_R_MODE_CHANGED)) show_mode(e, r->mode);
    if (r->flags & TWIME_R_RELEASED) {
        if (r->flags & TWIME_R_HANDLED) {
            g_free(g_queue_pop_head(&e->held));
        } else {
            forward_held(e);
        }
    }
    if ((r->flags & TWIME_R_HOLDING) && !e->release_timer) e->release_timer = g_timeout_add(10, release_tick, e);
    return r;
}

static gboolean handled(const twime_reply *r) { return r && (r->flags & TWIME_R_HANDLED); }

/* While keys are held: release what can be released. */
static gboolean release_tick(gpointer data) {
    TwEngine *e = data;
    const twime_reply *r;
    do {
        r = send(e, TWIME_EV_RELEASE, 0, 0, FALSE);
    } while (r && (r->flags & TWIME_R_RELEASED));
    if (r && (r->flags & TWIME_R_HOLDING)) return G_SOURCE_CONTINUE;
    /* No more held keys (or the library failed: whatever is left goes on, in order). */
    while (!g_queue_is_empty(&e->held)) forward_held(e);
    e->release_timer = 0;
    return G_SOURCE_REMOVE;
}

/* What the library needs to know about the client (twime_shell.h, TWIME_EV_CLIENT). Keys are held only when the client
 * takes forwarded keys in place of a key reported unhandled. A composition is never left to gnome-shell to commit on a
 * focus loss: on Wayland it is the client of every GTK program, and it commits into whatever field has the focus by
 * the time the commit arrives (LINUX-plan 7.7). */
static void tell_client(TwEngine *e) {
    uint32_t flags = 0;
    if (!(e->client_caps & IBUS_CAP_SYNC_PROCESS_KEY)) flags |= TWIME_CLIENT_DEFER;
    if (e->gnome_shell) flags |= TWIME_CLIENT_NO_FOCUS_COMMIT;
    send(e, TWIME_EV_CLIENT, flags, 0, FALSE);
}

static void sensitive(TwEngine *e) {
    guint purpose = IBUS_INPUT_PURPOSE_FREE_FORM, hints = 0;
    ibus_engine_get_content_type(IBUS_ENGINE(e), &purpose, &hints);
    /* Passwords and PINs, and any field whose text the application asks input methods not to keep. */
    send(e, TWIME_EV_SENSITIVE,
         purpose == IBUS_INPUT_PURPOSE_PASSWORD || purpose == IBUS_INPUT_PURPOSE_PIN || (hints & IBUS_INPUT_HINT_PRIVATE) != 0, 0, FALSE);
}

/* A character a text key types: a Unicode scalar value that is not a control character and not a blank.
 * (The same list as textChar in ../../fcitx5/src/twpotato.cpp: keep the two alike.) */
static gboolean text_char(guint32 u) {
    if (u < 0x21 || (u >= 0x7f && u <= 0xa0) || (u >= 0xd800 && u <= 0xdfff) || u > 0x10ffff) return FALSE;
    return !(u == 0x1680 || (u >= 0x2000 && u <= 0x200a) || u == 0x2028 || u == 0x2029 || u == 0x202f || u == 0x205f || u == 0x3000);
}

/* Key values that stand for a character, but not in the tables of both frameworks: for these one of IBus's and Fcitx5's
 * conversions gives a character and the other none (found by comparing the two over all values up to 0xffff; `blank`
 * differs between versions). Technical and box drawing pieces, a few marks, two Hangul letters, the euro sign: all text.
 * Named here so that the two shells agree. (The same list as listedText in ../../fcitx5/src/twpotato.cpp.) */
static gboolean listed_text(guint v) {
    return (v >= 0x8a1 && v <= 0x8a3) || (v >= 0x8a7 && v <= 0x8b0) || v == 0x8c9 || v == 0x9df || v == 0x9ef || v == 0x9f0 ||
           v == 0x9f2 || v == 0x9f3 || v == 0xaac || v == 0xaaf || v == 0xad5 || v == 0xef3 || v == 0xef9 || v == 0x13a4;
}

/* A key value that types text (a letter, digit or punctuation mark of any layout) - as opposed to the control,
 * function, modifier and number pad keys, and Space. Only text keys are placed by their key code. Decided by the
 * character IBus says the key value stands for; the dead keys and the Czech "ch" key values have none and are named
 * by their ranges in the key symbol definitions. (As typesText in the Fcitx5 shell.) */
static gboolean types_text(guint v) {
    if (v >= IBUS_KEY_KP_Space && v <= IBUS_KEY_KP_Equal) return FALSE; /* the number pad: digits, but not text keys */
    if ((v >= 0xfe50 && v <= 0xfe6f) || (v >= 0xfe80 && v <= 0xfe8c) || (v >= 0xfe90 && v <= 0xfe93)) return TRUE; /* dead_* */
    if (v >= 0xfea0 && v <= 0xfea5) return TRUE; /* ch, Ch, CH, c_h, C_h, C_H */
    if (listed_text(v)) return TRUE;
    return text_char(ibus_keyval_to_unicode(v));
}

/* Linux input event code (the numbering of linux/input-event-codes.h, which is IBus's key code) -> TWIME_KEY_*, for
 * the keys whose symbol depends on the keyboard layout: the letters, the digit row and the punctuation keys (47).
 * TWIME_KEY_OTHER: not one of these.
 * The same table is in ../../fcitx5/src/twpotato.cpp (keyAt): keep the two alike. */
static uint32_t key_at(guint code) {
    static const char digits[] = "1234567890"; /* KEY_1 (2) .. KEY_0 (11) */
    static const char top[] = "qwertyuiop";    /* KEY_Q (16) .. KEY_P (25) */
    static const char home[] = "asdfghjkl";    /* KEY_A (30) .. KEY_L (38) */
    static const char bottom[] = "zxcvbnm";    /* KEY_Z (44) .. KEY_M (50) */
    if (code >= 2 && code <= 11) return (uint32_t)digits[code - 2];
    if (code >= 16 && code <= 25) return (uint32_t)top[code - 16];
    if (code >= 30 && code <= 38) return (uint32_t)home[code - 30];
    if (code >= 44 && code <= 50) return (uint32_t)bottom[code - 44];
    switch (code) {
    case 12: return TWIME_KEY_MINUS;     /* KEY_MINUS */
    case 13: return TWIME_KEY_EQUALS;    /* KEY_EQUAL */
    case 26: return TWIME_KEY_LBRACKET;  /* KEY_LEFTBRACE */
    case 27: return TWIME_KEY_RBRACKET;  /* KEY_RIGHTBRACE */
    case 39: return TWIME_KEY_SEMICOLON; /* KEY_SEMICOLON */
    case 40: return TWIME_KEY_QUOTE;     /* KEY_APOSTROPHE */
    case 41: return TWIME_KEY_BACKQUOTE; /* KEY_GRAVE */
    case 43: return TWIME_KEY_BACKSLASH; /* KEY_BACKSLASH */
    case 51: return TWIME_KEY_COMMA;     /* KEY_COMMA */
    case 52: return TWIME_KEY_PERIOD;    /* KEY_DOT */
    case 53: return TWIME_KEY_SLASH;     /* KEY_SLASH */
    default: return TWIME_KEY_OTHER;
    }
}

/* The key of this event. The bopomofo layout is tied to the places of the keys, and the system layout may be another
 * one (Dvorak, Cyrillic, ...): a text key that comes with a key code is the key at that place of a US keyboard.
 * Everything else is what its key value says: keys that do not type text (Escape stays Escape whatever code comes
 * with it), keys outside the table, and clients that send no code. */
static uint32_t key_of_event(guint keyval, guint keycode) {
    uint32_t k = types_text(keyval) ? key_at(keycode) : TWIME_KEY_OTHER;
    return k != TWIME_KEY_OTHER ? k : key_of(keyval);
}

static gboolean tw_process_key_event(IBusEngine *engine, guint keyval, guint keycode, guint state) {
    TwEngine *e = (TwEngine *)engine;
    if (state & IBUS_FORWARD_MASK) return FALSE; /* one we sent on ourselves */
    uint32_t mods = mods_of(state);
    e->caps = mods & TWIME_MOD_CAPS;
    sensitive(e);
    const twime_reply *r = send(e, (state & IBUS_RELEASE_MASK) ? TWIME_EV_KEY_UP : TWIME_EV_KEY_DOWN, key_of_event(keyval, keycode), mods, FALSE);
    if (r && (r->flags & TWIME_R_HELD)) {
        HeldKey *k = g_new(HeldKey, 1);
        k->keyval = keyval;
        k->keycode = keycode;
        k->state = state;
        g_queue_push_tail(&e->held, k);
    }
    return handled(r);
}

static void tw_focus_in(IBusEngine *engine) {
    TwEngine *e = (TwEngine *)engine;
    tell_client(e);
    IBusPropList *props = ibus_prop_list_new();
    ibus_prop_list_append(props, e->mode);
    ibus_engine_register_properties(engine, props);
    sensitive(e);
    send(e, TWIME_EV_FOCUS_IN, 0, 0, TRUE);
}

/* Focus out, and the user switching to another input method: the same to the engine, which commits a composition
 * that can be committed and drops any other. The commit itself is done by the client (see draw and send). */
static void tw_focus_out(IBusEngine *engine) { send((TwEngine *)engine, TWIME_EV_FOCUS_OUT, 0, 0, FALSE); }
static void tw_disable(IBusEngine *engine) { send((TwEngine *)engine, TWIME_EV_FOCUS_OUT, 0, 0, FALSE); }

/* The client of the focus: "gnome-shell" for every program on GNOME Wayland that uses the compositor's text input
 * (the documented client names of IBusEngine::focus-in-id). */
static void tw_focus_in_id(IBusEngine *engine, const gchar *object_path, const gchar *client) {
    (void)object_path;
    ((TwEngine *)engine)->gnome_shell = g_strcmp0(client, "gnome-shell") == 0;
    tw_focus_in(engine);
}
/* A focus-in without a client name: ibus-daemon sends it when the engine is put on a context that already has the
 * focus (the user switching to this input method). In a Wayland session that context is most likely gnome-shell's:
 * it is treated as one. */
static void tw_focus_in_unnamed(IBusEngine *engine) {
    ((TwEngine *)engine)->gnome_shell = wayland_session;
    tw_focus_in(engine);
}
static void tw_focus_out_id(IBusEngine *engine, const gchar *object_path) {
    (void)object_path;
    tw_focus_out(engine);
}
static void tw_set_capabilities(IBusEngine *engine, guint caps) {
    TwEngine *e = (TwEngine *)engine;
    e->client_caps = caps;
    if (e->ctx) tell_client(e);
}
static void tw_reset(IBusEngine *engine) { send((TwEngine *)engine, TWIME_EV_RESET, 0, 0, FALSE); }

static void tw_set_content_type(IBusEngine *engine, guint purpose, guint hints) {
    (void)purpose;
    (void)hints;
    sensitive((TwEngine *)engine);
}

/* The panel's paging and cursor buttons: the same keys as on the keyboard, paging belongs to the engine. They carry
 * Caps Lock as the keyboard last reported it (the library remembers it from key events). */
static void panel_key(IBusEngine *engine, uint32_t key) { send((TwEngine *)engine, TWIME_EV_PANEL_KEY, key, ((TwEngine *)engine)->caps, FALSE); }
static void tw_page_up(IBusEngine *engine) { panel_key(engine, TWIME_KEY_PAGE_UP); }
static void tw_page_down(IBusEngine *engine) { panel_key(engine, TWIME_KEY_PAGE_DOWN); }
static void tw_cursor_up(IBusEngine *engine) { panel_key(engine, TWIME_KEY_UP); }
static void tw_cursor_down(IBusEngine *engine) { panel_key(engine, TWIME_KEY_DOWN); }

static void tw_candidate_clicked(IBusEngine *engine, guint index, guint button, guint state) {
    (void)button;
    (void)state;
    send((TwEngine *)engine, TWIME_EV_PICK, index, 0, FALSE);
}

static void tw_property_activate(IBusEngine *engine, const gchar *name, guint state) {
    (void)state;
    if (strcmp(name, MODE_PROPERTY) == 0) send((TwEngine *)engine, TWIME_EV_TOGGLE_MODE, 0, 0, FALSE);
}

static void tw_destroy(IBusObject *object) {
    TwEngine *e = (TwEngine *)object;
    if (e->release_timer) g_source_remove(e->release_timer);
    e->release_timer = 0;
    g_queue_clear_full(&e->held, g_free); /* the object and its client are gone */
    twime_ctx_free(e->ctx); /* closes the engine session */
    e->ctx = NULL;
    g_clear_object(&e->mode);
    IBUS_OBJECT_CLASS(tw_engine_parent_class)->destroy(object);
}

static void tw_engine_init(TwEngine *e) {
    e->ctx = NULL;
    e->caps = 0;
    g_queue_init(&e->held);
    e->release_timer = 0;
    e->gnome_shell = FALSE;
    e->client_caps = 0;
    e->mode = ibus_property_new(MODE_PROPERTY, PROP_TYPE_NORMAL, ibus_text_new_from_static_string("中文"), NULL,
                                ibus_text_new_from_static_string("中文 / 英文"), TRUE, TRUE, PROP_STATE_UNCHECKED, NULL);
    ibus_property_set_symbol(e->mode, ibus_text_new_from_static_string("中"));
    g_object_ref_sink(e->mode);
}

static void tw_engine_class_init(TwEngineClass *klass) {
    IBusEngineClass *engine = IBUS_ENGINE_CLASS(klass);
    IBUS_OBJECT_CLASS(klass)->destroy = tw_destroy;
    engine->process_key_event = tw_process_key_event;
    engine->focus_in = tw_focus_in_unnamed;
    engine->focus_out = tw_focus_out;
    engine->reset = tw_reset;
    engine->disable = tw_disable;
    engine->set_content_type = tw_set_content_type;
    engine->page_up = tw_page_up;
    engine->page_down = tw_page_down;
    engine->cursor_up = tw_cursor_up;
    engine->cursor_down = tw_cursor_down;
    engine->candidate_clicked = tw_candidate_clicked;
    engine->property_activate = tw_property_activate;
    engine->focus_in_id = tw_focus_in_id;
    engine->focus_out_id = tw_focus_out_id;
    engine->set_capabilities = tw_set_capabilities;
}

/* The factory's engine objects, made with "has-focus-id" (a construct-only property: ibus_factory_add_engine cannot
 * set it). */
static IBusEngine *create_engine(IBusFactory *factory, const gchar *engine_name, gpointer data) {
    static guint made;
    (void)data;
    if (g_strcmp0(engine_name, ENGINE_NAME) != 0) return NULL;
    gchar *path = g_strdup_printf("/org/freedesktop/IBus/Engine/TwPotato/%u", ++made);
    IBusEngine *engine = g_object_new(tw_engine_get_type(), "engine-name", engine_name, "object-path", path, "connection",
                                      ibus_service_get_connection(IBUS_SERVICE(factory)), "has-focus-id", TRUE, NULL);
    g_free(path);
    return engine;
}

static void on_disconnected(IBusBus *bus, gpointer data) {
    (void)bus;
    (void)data;
    ibus_quit();
}

int main(void) {
    const gchar *type = g_getenv("XDG_SESSION_TYPE"), *display = g_getenv("WAYLAND_DISPLAY");
    wayland_session = g_strcmp0(type, "wayland") == 0 || (display && *display);
    ibus_init();
    IBusBus *bus = ibus_bus_new();
    if (!ibus_bus_is_connected(bus)) {
        g_printerr("ibus-engine-twpotato: cannot connect to ibus-daemon\n");
        return 1;
    }
    if (twime_shell_open(&shell) != TWIME_OK) return 1;
    g_signal_connect(bus, "disconnected", G_CALLBACK(on_disconnected), NULL);
    IBusFactory *factory = ibus_factory_new(ibus_bus_get_connection(bus));
    g_signal_connect(factory, "create-engine", G_CALLBACK(create_engine), NULL);
    if (ibus_bus_request_name(bus, COMPONENT_NAME, 0) == 0) {
        g_printerr("ibus-engine-twpotato: cannot take the name " COMPONENT_NAME "\n");
        return 1;
    }
    ibus_main();
    /* ibus-daemon is gone. The engine objects go with the process; their sessions end with the connection. */
    return 0;
}
