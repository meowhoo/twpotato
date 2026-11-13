#!/bin/bash
# User-mode install of the Fcitx5 addon (development and tests): the two configuration files go into the user's Fcitx5
# data directory, and the addon file names the built module by its absolute path. Nothing is copied to system directories.
#
#     install-user.sh <cmake build directory> [<data home, default $XDG_DATA_HOME or ~/.local/share>]
#
# The engine and its assets are installed separately, under <data home>/twpotato/{bin,assets}.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
build=$(readlink -f "$1")
data=${2:-${XDG_DATA_HOME:-$HOME/.local/share}}
[ -f "$build/twpotato.so" ] || { echo "no twpotato.so in $build" >&2; exit 1; }
mkdir -p "$data/fcitx5/addon" "$data/fcitx5/inputmethod"
# Fcitx5 appends ".so" to Library.
sed "s|^Library=.*|Library=$build/twpotato|" "$build/twpotato-addon.conf" > "$data/fcitx5/addon/twpotato.conf"
cp "$here/twpotato.conf" "$data/fcitx5/inputmethod/twpotato.conf"
echo "installed for this user: $data/fcitx5/{addon,inputmethod}/twpotato.conf -> $build/twpotato.so"
echo "note: Fcitx5's stock hotkey takes a left Shift tap for itself (AltTriggerKeys=Shift_L), so only the right Shift"
echo "      switches Chinese / English. To change that see ime/linux/README.md; your Fcitx5 settings were not touched."
