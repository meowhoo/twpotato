/* Links libtwime_shell.a from C and drives the real engine through the product defaults (install root from
 * XDG_DATA_HOME, socket in XDG_RUNTIME_DIR). Run by build-and-run.sh, which prepares those directories.
 * Exit code 0: every check passed. */
#define _POSIX_C_SOURCE 200809L
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "twime_shell.h"

static int failures;

#define CHECK(cond) \
    do { \
        if (!(cond)) { \
            fprintf(stderr, "FAILED line %d: %s\n", __LINE__, #cond); \
            failures++; \
        } \
    } while (0)

static void nap(long ms) {
    struct timespec t = {ms / 1000, (ms % 1000) * 1000000L};
    nanosleep(&t, NULL);
}

static int is(twime_str s, const char *want) {
    return s.len == strlen(want) && (s.len == 0 || memcmp(s.ptr, want, s.len) == 0);
}

static const twime_reply *ev(twime_ctx *c, uint32_t kind, uint32_t a, uint32_t b) {
    const twime_reply *r = NULL;
    int status = twime_ctx_event(c, kind, a, b, &r);
    if (status != TWIME_OK || r == NULL) {
        fprintf(stderr, "twime_ctx_event(%u, %u, %u) -> %d\n", kind, a, b, status);
        exit(2);
    }
    return r;
}

static const twime_reply *key(twime_ctx *c, uint32_t k) {
    return ev(c, TWIME_EV_KEY_DOWN, k, 0);
}

/* Presses a key the engine never gets until the connection is there. */
static void wait_engine(twime_ctx *c) {
    for (int i = 0; i < 500; i++) {
        if (!(key(c, TWIME_KEY_OTHER)->flags & TWIME_R_NO_ENGINE)) return;
        nap(20);
    }
    fprintf(stderr, "no engine after 10 s\n");
    exit(2);
}

static void show(const char *what, const twime_reply *r) {
    printf("%s: flags=%u mode=%u commit=[%.*s] preedit=[%.*s] cursor=%u segments=%u", what, r->flags, r->mode,
           (int)r->commit.len, r->commit.len ? r->commit.ptr : "", (int)r->preedit.len, r->preedit.len ? r->preedit.ptr : "",
           r->cursor, r->n_segments);
    if (r->flags & TWIME_R_CANDIDATES_OPEN) {
        printf(" candidates(%u of %u, page %u, selected %u):", r->n_candidates, r->total, r->page, r->selected);
        for (uint32_t i = 0; i < r->n_candidates; i++)
            printf(" %u.%.*s", i + 1, (int)r->candidates[i].len, r->candidates[i].ptr);
    }
    printf("\n");
}

int main(void) {
    twime_shell *sh = NULL;
    twime_ctx *c = NULL;
    const twime_reply *r;

    CHECK(twime_shell_open(NULL) == TWIME_ERR_ARGUMENT);
    CHECK(twime_shell_open(&sh) == TWIME_OK && sh != NULL);
    CHECK(twime_ctx_new(NULL, &c) == TWIME_ERR_ARGUMENT);
    CHECK(twime_ctx_new(sh, NULL) == TWIME_ERR_ARGUMENT);
    CHECK(twime_ctx_new(sh, &c) == TWIME_OK && c != NULL);
    CHECK(twime_ctx_event(NULL, TWIME_EV_FOCUS_IN, 0, 0, &r) == TWIME_ERR_ARGUMENT);
    CHECK(twime_ctx_event(c, 99, 0, 0, &r) == TWIME_ERR_ARGUMENT);
    CHECK(twime_ctx_event(c, TWIME_EV_FOCUS_IN, 0, 0, NULL) == TWIME_OK);

    /* No engine yet: the first key passes and the engine is started behind it. */
    r = key(c, 's');
    show("first key", r);
    CHECK(!(r->flags & TWIME_R_HANDLED) && (r->flags & TWIME_R_NO_ENGINE));
    wait_engine(c);

    const char *keys = "su3cl3";
    for (const char *k = keys; *k; k++) {
        r = key(c, (uint32_t)*k);
        CHECK(r->flags & TWIME_R_HANDLED);
    }
    show("su3cl3", r);
    CHECK(is(r->preedit, "你好") && r->cursor == 2 && r->commit.len == 0);
    uint32_t bytes = 0, chars = 0;
    for (uint32_t i = 0; i < r->n_segments; i++) {
        bytes += r->segments[i].bytes;
        chars += r->segments[i].chars;
    }
    CHECK(bytes == r->preedit.len && chars == 2);

    r = key(c, TWIME_KEY_DOWN);
    show("down", r);
    CHECK((r->flags & TWIME_R_CANDIDATES_OPEN) && (r->flags & TWIME_R_UI_CHANGED) && r->n_candidates > 0);
    CHECK(r->selected < r->n_candidates && r->n_candidates <= r->per_page && r->total >= r->n_candidates);
    r = key(c, TWIME_KEY_ESC);
    CHECK(!(r->flags & TWIME_R_CANDIDATES_OPEN) && is(r->preedit, "你好"));
    r = key(c, TWIME_KEY_ENTER);
    show("enter", r);
    CHECK((r->flags & TWIME_R_HANDLED) && (r->flags & TWIME_R_UI_CHANGED) && is(r->commit, "你好") && r->preedit.len == 0);

    /* Shift tap. */
    r = ev(c, TWIME_EV_KEY_DOWN, TWIME_KEY_SHIFT_LEFT, 0);
    CHECK(!(r->flags & TWIME_R_HANDLED));
    r = ev(c, TWIME_EV_KEY_UP, TWIME_KEY_SHIFT_LEFT, TWIME_MOD_SHIFT);
    show("shift tap", r);
    CHECK(!(r->flags & TWIME_R_HANDLED) && (r->flags & TWIME_R_MODE_CHANGED) && r->mode == 1);
    CHECK(!(key(c, 's')->flags & TWIME_R_HANDLED));
    ev(c, TWIME_EV_KEY_DOWN, TWIME_KEY_SHIFT_LEFT, 0);
    r = ev(c, TWIME_EV_KEY_UP, TWIME_KEY_SHIFT_LEFT, TWIME_MOD_SHIFT);
    CHECK(r->mode == 0);

    /* The engine dies under a composition. SIGPIPE has its default action here (a C host): the process must live. */
    signal(SIGPIPE, SIG_DFL);
    key(c, 's');
    r = key(c, 'u');
    CHECK(r->preedit.len > 0);
    const char *kill_command = getenv("SMOKE_KILL_ENGINE");
    CHECK(kill_command != NULL && system(kill_command) == 0);
    nap(200);
    r = key(c, '3');
    show("after kill", r);
    CHECK(!(r->flags & TWIME_R_HANDLED) && (r->flags & TWIME_R_NO_ENGINE) && (r->flags & TWIME_R_UI_CHANGED));
    CHECK(r->preedit.len == 0 && r->commit.len == 0 && !(r->flags & TWIME_R_CANDIDATES_OPEN));
    wait_engine(c);
    for (const char *k = "cl3"; *k; k++) key(c, (uint32_t)*k);
    r = key(c, TWIME_KEY_ENTER);
    show("after restart", r);
    CHECK(is(r->commit, "好"));

    CHECK(twime_ctx_free(c) == TWIME_OK);
    CHECK(twime_ctx_free(NULL) == TWIME_OK);
    CHECK(twime_shell_close(sh) == TWIME_OK);
    CHECK(twime_shell_close(NULL) == TWIME_OK);
    printf(failures ? "smoke: %d FAILED\n" : "smoke: all checks passed\n", failures);
    return failures ? 1 : 0;
}
