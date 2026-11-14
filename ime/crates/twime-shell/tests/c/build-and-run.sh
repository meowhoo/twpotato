#!/bin/bash
# Builds libtwime_shell.a, links tests/c/smoke.c against it and runs it with the real engine, through the product's
# default paths in a private directory (user-mode install root, runtime directory, HOME). Linux.
#
#     tests/c/build-and-run.sh <twime-engined> <assets dir>
#
# Uses CARGO_TARGET_DIR (default: the workspace's target) and JOBS (default 3).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
crate=$(cd "$here/../.." && pwd)
engine=$(readlink -f "$1")
assets=$(readlink -f "$2")
target=${CARGO_TARGET_DIR:-$crate/../../target}

(cd "$crate" && cargo build --release --locked -j "${JOBS:-3}" -p twime-shell)

stage=$(mktemp -d /tmp/twsh-c-XXXXXX)
mkdir -m 700 "$stage/home" "$stage/run" "$stage/data" "$stage/config" "$stage/state"
mkdir -p "$stage/data/twpotato/bin"
ln -s "$engine" "$stage/data/twpotato/bin/twime-engined"
ln -s "$assets" "$stage/data/twpotato/assets"
installed="$stage/data/twpotato/bin/twime-engined"
# The engine's command line starts with this path, which nothing else on the machine has.
kill_engine="pkill -KILL -f ^$installed"

cleanup() {
    $kill_engine 2>/dev/null || true
    rm -rf "$stage"
}
trap cleanup EXIT

# The system libraries a C or C++ program adds after libtwime_shell.a (what rustc reports as native-static-libs).
libs="-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc"
cc -std=c11 -Wall -Wextra -Werror -I "$crate/include" "$here/smoke.c" "$target/release/libtwime_shell.a" $libs -o "$stage/smoke"
if command -v c++ >/dev/null; then
    echo '#include "twime_shell.h"' | c++ -std=c++17 -Wall -Wextra -Werror -I "$crate/include" -x c++ -fsyntax-only - && echo "header: valid C++"
fi

env -i PATH="$PATH" HOME="$stage/home" XDG_RUNTIME_DIR="$stage/run" XDG_DATA_HOME="$stage/data" \
    XDG_CONFIG_HOME="$stage/config" XDG_STATE_HOME="$stage/state" SMOKE_KILL_ENGINE="$kill_engine" "$stage/smoke"

$kill_engine 2>/dev/null || true
sleep 0.3
if pgrep -f "^$installed" >/dev/null; then
    echo "an engine of this run is still there" >&2
    exit 1
fi
echo "no engine left"
