/* twime_shell.h - C interface of libtwime_shell.a (crate twime-shell).
 *
 * Everything the front ends (Fcitx5, IBus, macOS) share: the connection to the engine twime-engined (find it, start it,
 * reconnect), sessions, the Shift single tap, the sensitive-field state. The front end only translates: framework
 * events into twime_ctx_event calls, and each reply into the framework's commit / preedit / candidate calls.
 *
 * Rules
 * - One twime_shell per process (it holds the one engine connection). One twime_ctx per input context object of the
 *   framework (it maps to one engine session).
 * - Not thread safe: call everything of one shell, including its contexts, from one thread.
 * - Every function returns a status code (TWIME_OK = 0). Nothing here blocks longer than the call time limit (30 ms
 *   per event); the engine is started in the background and never waited for. Keys that come while it starts may be
 *   held instead (TWIME_R_HELD): the front end keeps the framework's events and sends them on later (see "Holding").
 * - Strings are UTF-8, pointer + length in bytes, NOT zero-terminated. Do not read ptr when len is 0.
 * - The reply of twime_ctx_event belongs to the library and stays valid until the next twime_ctx_event on the same
 *   context or its twime_ctx_free.
 * - Link with: libtwime_shell.a -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc (Linux, what rustc reports as
 *   native-static-libs; see the crate's tests/c/build-and-run.sh). Build the library with panic = "unwind" (the
 *   workspace profile), or a caught internal error aborts the host process instead.
 *
 * This interface is not final before all three front ends have used it.
 */
#ifndef TWIME_SHELL_H
#define TWIME_SHELL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- status codes ---- */
#define TWIME_OK 0
#define TWIME_ERR_ARGUMENT 1 /* null pointer, unknown event kind, value out of range; nothing was done */
#define TWIME_ERR_INTERNAL 2 /* a bug inside the library was caught; treat the key as not handled and clear the display */

/* ---- keys: the position on a US keyboard, never the character produced ---- */
/* Letters: 'a'..'z' (97..122). Main-row digits: '0'..'9' (48..57). */
#define TWIME_KEY_OTHER 0 /* any key not listed here (the modifier keys themselves: TWIME_KEY_MODIFIER) */
#define TWIME_KEY_MINUS 1
#define TWIME_KEY_COMMA 2
#define TWIME_KEY_PERIOD 3
#define TWIME_KEY_SLASH 4
#define TWIME_KEY_SEMICOLON 5
#define TWIME_KEY_SPACE 6
#define TWIME_KEY_ENTER 7
#define TWIME_KEY_ESC 8
#define TWIME_KEY_BACKSPACE 9
#define TWIME_KEY_DELETE 10
#define TWIME_KEY_TAB 11
#define TWIME_KEY_LEFT 12
#define TWIME_KEY_RIGHT 13
#define TWIME_KEY_UP 14
#define TWIME_KEY_DOWN 15
#define TWIME_KEY_HOME 16
#define TWIME_KEY_END 17
#define TWIME_KEY_PAGE_UP 18
#define TWIME_KEY_PAGE_DOWN 19
#define TWIME_KEY_BACKQUOTE 20
#define TWIME_KEY_LBRACKET 21
#define TWIME_KEY_RBRACKET 22
#define TWIME_KEY_QUOTE 23
#define TWIME_KEY_BACKSLASH 24
#define TWIME_KEY_EQUALS 25
#define TWIME_KEY_NUMPAD_DECIMAL 26
#define TWIME_KEY_NUMPAD_0 30 /* number pad 0..9 = 30..39 */
#define TWIME_KEY_NUMPAD_ADD 40
#define TWIME_KEY_NUMPAD_SUBTRACT 41
#define TWIME_KEY_NUMPAD_MULTIPLY 42
#define TWIME_KEY_NUMPAD_DIVIDE 43
#define TWIME_KEY_SHIFT_LEFT 200
#define TWIME_KEY_SHIFT_RIGHT 201
#define TWIME_KEY_MODIFIER 202 /* Ctrl, Alt, Super, Meta, Hyper, AltGr, Caps / Num / Shift Lock themselves: as
                                * TWIME_KEY_OTHER, but never a reason to finish a composition (TWIME_CLIENT_NO_FOCUS_COMMIT) */

/* ---- modifiers held / locked while the key event happens (bit set) ---- */
#define TWIME_MOD_SHIFT 1
#define TWIME_MOD_CTRL 2
#define TWIME_MOD_ALT 4
#define TWIME_MOD_SUPER 8
#define TWIME_MOD_CAPS 16 /* Caps Lock is on */

/* ---- event kinds of twime_ctx_event(ctx, kind, a, b, &reply) ---- */
#define TWIME_EV_KEY_DOWN 1  /* a = TWIME_KEY_*, b = TWIME_MOD_* bits. Send every key press, the Shift keys too. */
#define TWIME_EV_KEY_UP 2    /* a = TWIME_KEY_*, b = TWIME_MOD_* bits. Send every key release. Never handled. */
#define TWIME_EV_FOCUS_IN 3  /* the context got the input focus */
#define TWIME_EV_FOCUS_OUT 4 /* the context lost the input focus */
#define TWIME_EV_RESET 5     /* the framework asks for a reset (Fcitx5 reset; IBus reset): a preedit is discarded without
                              * a commit, and the engine forgets the text typed before, with or without a preedit */
#define TWIME_EV_SENSITIVE 6 /* a = 1: the context is a password / PIN / otherwise sensitive field now; a = 0: it is not */
#define TWIME_EV_PICK 7      /* a = index of the clicked candidate within reply->candidates */
#define TWIME_EV_TOGGLE_MODE 8 /* the Chinese / English indicator was clicked: switches the mode (not in a sensitive
                              * field, not with Caps Lock on, not before the first key reached the engine) */
#define TWIME_EV_CLIENT 9    /* a = TWIME_CLIENT_* bits: what the client behind this context can do. Send it when the
                              * context is made and whenever it may have changed (IBus: at every focus-in). A new
                              * context is 0: no keys held, compositions committed on a focus loss. */
#define TWIME_EV_RELEASE 10  /* release held keys: send it while the previous reply had TWIME_R_HOLDING (every 10 ms
                              * or so), and keep sending it as long as the reply has TWIME_R_RELEASED */

#define TWIME_EV_PANEL_KEY 11 /* a = TWIME_KEY_*, b = TWIME_MOD_* bits: a key the panel sends for a click on its page
                              * or cursor buttons. As TWIME_EV_KEY_DOWN, but never held (the front end has no
                              * framework event to keep for it); held keys go on first */

/* ---- TWIME_EV_CLIENT bits ---- */
#define TWIME_CLIENT_DEFER 1 /* keys may be held and sent on to the client later (IBus: not with IBUS_CAP_SYNC_PROCESS_KEY) */
#define TWIME_CLIENT_NO_FOCUS_COMMIT 2 /* a composition the client commits when the focus leaves may arrive in the field
                              * that has the focus by then (IBus through gnome-shell on Wayland). Then a composition is
                              * finished before any key the input method passes on (that key may move the focus) and
                              * dropped on a focus loss; TWIME_R_COMMITTABLE is never set. */

/* ---- reply ---- */
typedef struct twime_str {
    const char *ptr; /* UTF-8, not zero-terminated; unspecified when len is 0 */
    size_t len;      /* bytes */
} twime_str;

#define TWIME_SEG_LOCKED 1      /* the user fixed this part with a candidate */
#define TWIME_SEG_PENDING 2     /* bopomofo still being typed */
#define TWIME_SEG_UNCONVERTED 4 /* a syllable without a character, shown as typed */
#define TWIME_SEG_RUN 8         /* part of an English run */

/* One stretch of the preedit; the segments in order make up the whole preedit text. */
typedef struct twime_segment {
    uint32_t bytes; /* length of this segment in the preedit, in bytes */
    uint32_t chars; /* the same in characters (Unicode scalar values) */
    uint32_t flags; /* TWIME_SEG_* */
} twime_segment;

#define TWIME_R_HANDLED 1         /* the key is ours: do not pass it to the application. Clear: pass it on. */
#define TWIME_R_UI_CHANGED 2      /* preedit / candidates differ from this context's previous reply: show them again */
#define TWIME_R_CANDIDATES_OPEN 4 /* the candidate list is to be shown */
#define TWIME_R_MODE_CHANGED 8    /* mode differs from this context's previous reply */
#define TWIME_R_NO_ENGINE 16      /* this key needed the engine and there was none (starting, gone, too slow); it passes */
#define TWIME_R_COMMITTABLE 32    /* there is a preedit and TWIME_EV_FOCUS_OUT now would commit exactly its text (clear:
                                   * it would be dropped). For frameworks where the client, not the input method,
                                   * commits the preedit on a focus loss (IBus). Part of what UI_CHANGED covers. */
#define TWIME_R_HELD 64           /* the key is held: report it handled, keep the framework's event at the end of this
                                   * context's queue, and send TWIME_EV_RELEASE while TWIME_R_HOLDING */
#define TWIME_R_RELEASED 128      /* (TWIME_EV_RELEASE) the oldest held key went to the engine and this is its reply:
                                   * apply it; then, unless TWIME_R_HANDLED, send that event on to the application.
                                   * Ask TWIME_EV_RELEASE again at once. */
#define TWIME_R_HOLDING 256       /* keys are still held */

typedef struct twime_reply {
    uint32_t flags; /* TWIME_R_* */
    uint32_t mode;  /* 0 Chinese, 1 English; as last heard from the engine (0 before the first key reached it) */
    twime_str commit;  /* len > 0: commit this text first */
    twime_str preedit; /* the whole preedit to show after that; len 0: none */
    uint32_t cursor;   /* caret in the preedit, in CHARACTERS (Unicode scalar values) from its start */
    uint32_t n_segments;
    const twime_segment *segments;
    const twime_str *candidates; /* the current page only; valid while TWIME_R_CANDIDATES_OPEN */
    uint32_t n_candidates;
    uint32_t selected; /* index of the highlighted candidate within the page */
    uint32_t page;     /* number of this page, from 0 */
    uint32_t total;    /* candidates in all pages */
    uint32_t per_page; /* candidates per page */
    uint32_t release_before; /* first of all: send this many of the oldest held events on to the application unchanged */
} twime_reply;

typedef struct twime_shell twime_shell;
typedef struct twime_ctx twime_ctx;

/* Creates the shell. Does not connect and does not start the engine (the first focus-in or key does). */
int twime_shell_open(twime_shell **out);

/* Ends the shell. Free its contexts first (a context freed later is still freed safely). NULL is allowed. */
int twime_shell_close(twime_shell *shell);

/* Creates the state for one input context object. No engine session exists before its first key. */
int twime_ctx_new(twime_shell *shell, twime_ctx **out);

/* The input context object is gone: closes its engine session and frees it and its reply. NULL is allowed. */
int twime_ctx_free(twime_ctx *ctx);

/* Holding. The engine is started when a context gets the focus (TWIME_EV_FOCUS_IN). A key that comes before it
 * answers, in a context whose client allows it (TWIME_CLIENT_DEFER), is held: TWIME_R_HELD. The front end keeps a
 * queue of the framework's key events per context, in order, and sends them on to the application itself (IBus
 * ibus_engine_forward_key_event, Fcitx5 InputContext::forwardKey) when a reply says so: release_before (unchanged,
 * at once), or TWIME_R_RELEASED without TWIME_R_HANDLED (after the engine saw it). Held keys are released to the
 * engine once it answers, or all go on unchanged after 2 seconds or as soon as no engine is coming; any other event
 * that changes the context (focus, reset, ...) first sends them on unchanged. Nothing is lost and the order is kept.
 *
 * Handles one event (TWIME_EV_*) and gives the reply. Apply every reply in this order:
 *   0. release_before > 0: send that many held events on to the application, oldest first.
 *   1. commit.len > 0: commit the text.
 *   2. TWIME_R_UI_CHANGED: replace the shown preedit and candidate list by the ones of this reply (an empty preedit
 *      and no TWIME_R_CANDIDATES_OPEN mean: remove them). This is also how a lost engine clears the display.
 *   3. TWIME_R_MODE_CHANGED: update the Chinese / English indicator from mode.
 *   4. Key events: return "handled" to the framework exactly when TWIME_R_HANDLED is set (TWIME_R_HELD: also set;
 *      keep the event). TWIME_EV_RELEASE: see TWIME_R_RELEASED.
 * out may be NULL when the reply is not wanted. */
int twime_ctx_event(twime_ctx *ctx, uint32_t kind, uint32_t a, uint32_t b, const twime_reply **out);

#ifdef __cplusplus
}
#endif

#endif /* TWIME_SHELL_H */
