#!/bin/bash
# Headless tests of the Fcitx5 addon (LINUX-plan 7.2): real fcitx5, real addon, real engine; the client is a
# test program on fcitx5's D-Bus interface. Everything runs in a private session bus with its own HOME and XDG
# directories; no display is used (the GTK part starts its own Xvfb). Repeatable: every run makes a new stage directory.
#
#     run.sh [func[:NAME]] [cases] [gtk]          (no argument: all three; func:NAME runs the tests whose name contains NAME)
#
# Environment (defaults are the test VM's layout):
#   TWP_ADDON   cmake build directory of the addon          (default $CARGO_TARGET_DIR/linux/fcitx5)
#   TWP_ENGINE  twime-engined                               (default $CARGO_TARGET_DIR/release/twime-engined)
#   TWP_ASSETS  assets directory                            (default ~/twp/assets/r8a)
#   TWP_CASES   input set, the format of ime/xcmp           (default ~/twp/ref/cases-22421.tsv)
#   TWP_XCMP    the xcmp tool, for the engine-level reference (default ~/twp/target-xcmp/release/xcmp)
#   TWP_OUT     results and the cached reference            (default ~/twp/out/fcitx5)
#   CASES_LIMIT run only the first N cases of the set       (default: all)
# Exit code 0: every part that ran passed and no process of the run is left.
set -u
here=$(cd "$(dirname "$0")" && pwd)
target=${CARGO_TARGET_DIR:-$HOME/twp/target}
addon=$(readlink -f "${TWP_ADDON:-$target/linux/fcitx5}")
engine=$(readlink -f "${TWP_ENGINE:-$target/release/twime-engined}")
assets=$(readlink -f "${TWP_ASSETS:-$HOME/twp/assets/r8a}")
cases=${TWP_CASES:-$HOME/twp/ref/cases-22421.tsv}
xcmp=${TWP_XCMP:-$HOME/twp/target-xcmp/release/xcmp}
out=${TWP_OUT:-$HOME/twp/out/fcitx5}
[ $# -gt 0 ] || set -- func cases gtk
for f in "$addon/twpotato.so" "$engine" "$assets/model.bin"; do
    [ -e "$f" ] || { echo "missing: $f" >&2; exit 2; }
done
[ "$(id -u)" != 0 ] || { echo "do not run as root" >&2; exit 2; }
mkdir -p "$out"

stage=$(mktemp -d /tmp/twfx-XXXXXX)
for d in home run data config state cache; do mkdir -m 700 "$stage/$d"; done

# User-mode install into the stage: addon (absolute path), engine, assets.
bash "$here/../install-user.sh" "$addon" "$stage/data" >/dev/null || exit 2
mkdir -p "$stage/data/twpotato/bin"
ln -s "$assets" "$stage/data/twpotato/assets"
# The installed "engine" is a wrapper that records every start (the tests count them).
cat > "$stage/data/twpotato/bin/twime-engined" <<WRAP
#!/bin/sh
date +%s.%N >> "$stage/engine-starts"
exec "$engine" "\$@"
WRAP
chmod +x "$stage/data/twpotato/bin/twime-engined"

# fcitx5: one group with the keyboard layout and twpotato; new input contexts start with twpotato active.
mkdir -p "$stage/config/fcitx5"
cat > "$stage/config/fcitx5/profile" <<'PROFILE'
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=twpotato

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[Groups/0/Items/1]
Name=twpotato
Layout=

[GroupOrder]
0=Default
PROFILE
# ($stage/config/fcitx5/config is written by the tests: fx.write_fcitx_config.)

# Every process of the run carries TWFX_STAGE in its environment: that is how leftovers are found.
leftovers() {
    for p in /proc/[0-9]*; do
        if { tr '\0' '\n' < "$p/environ"; } 2>/dev/null | grep -q -x "TWFX_STAGE=$stage"; then
            echo "${p#/proc/} $(tr '\0' ' ' < "$p/cmdline" 2>/dev/null | cut -c1-120)"
        fi
    done
}

env -i PATH="$PATH" LANG=C.UTF-8 HOME="$stage/home" XDG_RUNTIME_DIR="$stage/run" XDG_DATA_HOME="$stage/data" \
    XDG_CONFIG_HOME="$stage/config" XDG_STATE_HOME="$stage/state" XDG_CACHE_HOME="$stage/cache" \
    TWFX_STAGE="$stage" TWFX_ENGINE="$engine" TWFX_ASSETS="$assets" TWFX_CASES="$cases" TWFX_XCMP="$xcmp" TWFX_OUT="$out" \
    CASES_LIMIT="${CASES_LIMIT:-}" \
    dbus-run-session -- sh -c 'exec python3 "$@" 2>&3' sh "$here/main.py" "$@" 3>&2 2>"$stage/bus.log"
status=$?

# The tests stop every fcitx5 they start. An engine outlives its fcitx5 by design (LINUX-plan 4.3): the tests
# end theirs too. Anything still here is a failure of the run.
sleep 0.3
left=$(leftovers)
if [ -n "$left" ]; then
    echo "LEFTOVER PROCESSES of this run:"; echo "$left"
    echo "$left" | while read -r pid _; do kill -KILL "$pid" 2>/dev/null; done
    status=1
else
    echo "no process of this run is left"
fi
if [ $status = 0 ] && [ -z "${KEEP_STAGE:-}" ]; then rm -rf "$stage"; else echo "stage kept: $stage"; fi
echo "run.sh: exit $status"
exit $status
