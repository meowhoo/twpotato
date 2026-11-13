#!/bin/bash
# Headless tests of the IBus shell (LINUX-plan 7.2): real ibus-daemon, real ibus-engine-twpotato, real engine;
# the test program is both the client and the panel, on ibus-daemon's own bus. Everything runs in a private session bus
# with its own HOME and XDG directories; no display is used (the GTK part starts its own Xvfb). Repeatable: every run
# makes a new stage directory.
#
#     run.sh [component] [func[:NAME]] [cases] [gtk] [both]
#       no argument: component func cases gtk. func:NAME runs the tests whose name contains NAME.
#       component: the component files cmake writes for directories with odd names (only cmake's configure step).
#       both: assertion 8, fcitx5 and ibus-daemon side by side on one engine (needs the Fcitx5 addon built too).
#
# Environment (defaults are the test VM's layout):
#   TWP_IBUS    cmake build directory of the IBus shell     (default $CARGO_TARGET_DIR/linux/ibus)
#   TWP_ADDON   cmake build directory of the Fcitx5 addon, for "both" (default $CARGO_TARGET_DIR/linux/fcitx5)
#   TWP_ENGINE  twime-engined                               (default $CARGO_TARGET_DIR/release/twime-engined)
#   TWP_ASSETS  assets directory                            (default ~/twp/assets/r8a)
#   TWP_CASES   input set, the format of ime/xcmp           (default ~/twp/ref/cases-22421.tsv)
#   TWP_XCMP    the xcmp tool, for the engine-level reference (default ~/twp/target-xcmp/release/xcmp)
#   TWP_OUT     results and the cached reference            (default ~/twp/out/ibus)
#   CASES_LIMIT run only the first N cases of the set       (default: all)
# Exit code 0: every part that ran passed and no process of the run is left.
set -u
here=$(cd "$(dirname "$0")" && pwd)
target=${CARGO_TARGET_DIR:-$HOME/twp/target}
shell=$(readlink -f "${TWP_IBUS:-$target/linux/ibus}")
addon=$(readlink -f "${TWP_ADDON:-$target/linux/fcitx5}")
engine=$(readlink -f "${TWP_ENGINE:-$target/release/twime-engined}")
assets=$(readlink -f "${TWP_ASSETS:-$HOME/twp/assets/r8a}")
cases=${TWP_CASES:-$HOME/twp/ref/cases-22421.tsv}
xcmp=${TWP_XCMP:-$HOME/twp/target-xcmp/release/xcmp}
out=${TWP_OUT:-$HOME/twp/out/ibus}
[ $# -gt 0 ] || set -- component func cases gtk
for f in "$shell/ibus-engine-twpotato" "$shell/twpotato.xml" "$engine" "$assets/model.bin"; do
    [ -e "$f" ] || { echo "missing: $f" >&2; exit 2; }
done
[ "$(id -u)" != 0 ] || { echo "do not run as root" >&2; exit 2; }
mkdir -p "$out"
# The engine-level reference is the same for both frameworks: take what a Fcitx5 run left next door (it is checked
# for freshness where it is used).
for f in "$(dirname "$out")"/fcitx5/ref-*; do
    if [ -e "$f" ] && [ ! -e "$out/$(basename "$f")" ]; then cp -p "$f" "$out/"; fi
done

stage=$(mktemp -d /tmp/twib-XXXXXX)
for d in home run data config state cache; do mkdir -m 700 "$stage/$d"; done

# The components ibus-daemon may see: ours (the component file of the build directory names the executable there by
# its absolute path) and the keyboard layouts (ibus-engine-simple), to have another engine to switch to.
mkdir "$stage/component"
cp "$shell/twpotato.xml" /usr/share/ibus/component/simple.xml "$stage/component/" || exit 2

# User-mode install of engine and assets into the stage. The installed "engine" is a wrapper that records every start
# (the tests count them).
mkdir -p "$stage/data/twpotato/bin"
ln -s "$assets" "$stage/data/twpotato/assets"
cat > "$stage/data/twpotato/bin/twime-engined" <<WRAP
#!/bin/sh
date +%s.%N >> "$stage/engine-starts"
exec "$engine" "\$@"
WRAP
chmod +x "$stage/data/twpotato/bin/twime-engined"

# For "both": the Fcitx5 addon and fcitx5's profile, as fcitx5/test/run.sh makes them.
if [ -f "$addon/twpotato.so" ]; then
    bash "$here/../../fcitx5/install-user.sh" "$addon" "$stage/data" >/dev/null || exit 2
    mkdir -p "$stage/config/fcitx5"
    cat > "$stage/config/fcitx5/profile" <<PROFILE
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
fi

# Every process of the run carries TWFX_STAGE in its environment: that is how leftovers are found.
leftovers() {
    for p in /proc/[0-9]*; do
        if { tr '\0' '\n' < "$p/environ"; } 2>/dev/null | grep -q -x "TWFX_STAGE=$stage"; then
            echo "${p#/proc/} $(tr '\0' ' ' < "$p/cmdline" 2>/dev/null | cut -c1-120)"
        fi
    done
}

# GSETTINGS_BACKEND=keyfile: whatever reads settings in this run reads a file in the stage, never the desktop's dconf.
# The variable names TWFX_* are the ones fcitx5/test uses: its helpers are shared.
env -i PATH="$PATH" LANG=C.UTF-8 HOME="$stage/home" XDG_RUNTIME_DIR="$stage/run" XDG_DATA_HOME="$stage/data" \
    XDG_CONFIG_HOME="$stage/config" XDG_STATE_HOME="$stage/state" XDG_CACHE_HOME="$stage/cache" \
    IBUS_COMPONENT_PATH="$stage/component" GSETTINGS_BACKEND=keyfile \
    TWFX_STAGE="$stage" TWFX_ENGINE="$engine" TWFX_ASSETS="$assets" TWFX_CASES="$cases" TWFX_XCMP="$xcmp" TWFX_OUT="$out" \
    TWFX_SHELL_LIB="$target/release/libtwime_shell.a" \
    CASES_LIMIT="${CASES_LIMIT:-}" \
    dbus-run-session -- sh -c 'exec python3 "$@" 2>&3' sh "$here/main.py" "$@" 3>&2 2>"$stage/bus.log"
status=$?

# The tests stop every ibus-daemon they start, and ibus-engine-twpotato ends with it. An engine outlives its shell by
# design (LINUX-plan 4.3): the tests end theirs too. Anything still here is a failure of the run.
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
