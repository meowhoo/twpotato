#!/bin/bash
# Builds everything the Linux version consists of, from this source tree: the engine twime-engined, the shared front-end
# library libtwime_shell.a, the Fcitx5 addon twpotato.so and the IBus engine program ibus-engine-twpotato.
#
#     ime/linux/build.sh
#
# CARGO_TARGET_DIR: where cargo builds (default ime/target). TWP_BUILD: where the addon is built (default
# $CARGO_TARGET_DIR/linux). JOBS: parallel jobs (default 3). TWP_PREFIX: the install prefix cmake is configured with
# (package.sh sets /usr; default: cmake's own). It decides where "cmake --install" puts the files and which path the
# installed IBus component file names; what is built is the same.
# Needs: the Rust toolchain of rust-toolchain.toml, cmake, pkg-config, gcc, g++, libfcitx5core-dev, libibus-1.0-dev.
set -eu
ime=$(cd "$(dirname "$0")/.." && pwd)
# Absolute, and the same for cargo (which runs in ime/) and cmake: a relative CARGO_TARGET_DIR is taken from the
# directory this script was started in.
target=$(readlink -m "${CARGO_TARGET_DIR:-$ime/target}")
build=$(readlink -m "${TWP_BUILD:-$target/linux}")
export CARGO_TARGET_DIR=$target
jobs=${JOBS:-3}
prefix=()
[ -z "${TWP_PREFIX:-}" ] || prefix=("-DCMAKE_INSTALL_PREFIX=$TWP_PREFIX")

(cd "$ime" && cargo build --release --locked -j "$jobs" -p twime-engined -p twime-shell)
cmake -S "$ime/linux/fcitx5" -B "$build/fcitx5" -DCMAKE_BUILD_TYPE=Release ${prefix[@]+"${prefix[@]}"} -DTWIME_SHELL_LIB="$target/release/libtwime_shell.a"
cmake --build "$build/fcitx5" -j "$jobs"
cmake -S "$ime/linux/ibus" -B "$build/ibus" -DCMAKE_BUILD_TYPE=Release ${prefix[@]+"${prefix[@]}"} -DTWIME_SHELL_LIB="$target/release/libtwime_shell.a"
cmake --build "$build/ibus" -j "$jobs"

echo "engine: $target/release/twime-engined"
echo "shell:  $target/release/libtwime_shell.a"
echo "fcitx5: $build/fcitx5/twpotato.so"
echo "ibus:   $build/ibus/ibus-engine-twpotato (component file: $build/ibus/twpotato.xml)"
