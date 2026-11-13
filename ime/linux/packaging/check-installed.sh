#!/bin/bash
# Checks the installed packages of TwPotato (the system install: /usr/lib/twpotato, /usr/share/twpotato, the Fcitx5
# addon directory, /usr/share/ibus/component) by typing through them, headless: a private session bus, its own HOME and
# XDG directories with no user-mode install in them, the frameworks' real daemons, the test clients of
# ../fcitx5/test and ../ibus/test. Nothing of the desktop session is touched.
#
#     check-installed.sh [ibus] [fcitx5]        (no argument: both)
#
# For each framework: "su3cl3" + Enter arrives as one commit of 你好; the engine that served it is
# /usr/lib/twpotato/twime-engined with /usr/share/twpotato/assets; the front end is the installed file.
# Needs python3-gi; for ibus gir1.2-ibus-1.0; for fcitx5 nothing more. Exit code 0: all checks passed, no process left.
set -u
here=$(cd "$(dirname "$0")" && pwd)
[ $# -gt 0 ] || set -- ibus fcitx5
[ "$(id -u)" != 0 ] || { echo "do not run as root" >&2; exit 2; }

status=0
for part in "$@"; do
    case $part in ibus | fcitx5) ;; *) echo "unknown part: $part" >&2; exit 2 ;; esac
    stage=$(mktemp -d /tmp/twchk-XXXXXX)
    for d in home run data config state cache; do mkdir -m 700 "$stage/$d"; done
    if [ "$part" = fcitx5 ]; then  # one group with the keyboard layout and twpotato, as a user would set it up
        mkdir -p "$stage/config/fcitx5"
        printf '%s\n' '[Groups/0]' 'Name=Default' 'Default Layout=us' 'DefaultIM=twpotato' '' '[Groups/0/Items/0]' \
            'Name=keyboard-us' 'Layout=' '' '[Groups/0/Items/1]' 'Name=twpotato' 'Layout=' '' '[GroupOrder]' '0=Default' \
            > "$stage/config/fcitx5/profile"
    fi
    # GSETTINGS_BACKEND=keyfile: settings are read from the stage, never from the desktop's dconf.
    env -i PATH="$PATH" LANG=C.UTF-8 HOME="$stage/home" XDG_RUNTIME_DIR="$stage/run" XDG_DATA_HOME="$stage/data" \
        XDG_CONFIG_HOME="$stage/config" XDG_STATE_HOME="$stage/state" XDG_CACHE_HOME="$stage/cache" \
        GSETTINGS_BACKEND=keyfile TWFX_STAGE="$stage" \
        dbus-run-session -- sh -c 'exec python3 "$@" 2>&3' sh "$here/check-installed.py" "$part" 3>&2 2>"$stage/bus.log"
    code=$?
    sleep 0.3
    left=""
    for p in /proc/[0-9]*; do  # every process of the run carries TWFX_STAGE in its environment
        if { tr '\0' '\n' < "$p/environ"; } 2>/dev/null | grep -q -x "TWFX_STAGE=$stage"; then
            left="$left ${p#/proc/}"
        fi
    done
    if [ -n "$left" ]; then
        echo "$part: LEFTOVER PROCESSES:$left"; kill -KILL $left 2>/dev/null; code=1
    fi
    if [ $code = 0 ]; then echo "$part: passed"; rm -rf "$stage"; else echo "$part: FAILED (stage kept: $stage)"; status=1; fi
done
echo "check-installed.sh: exit $status"
exit $status
