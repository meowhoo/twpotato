#!/bin/sh
# Builds build/TwPotato.app (ad-hoc signed): the input method, the engine and the assets in one bundle.
#   TWP_ASSETS=<directory of the model assets> ime/macos/build.sh
# Needs the Command Line Tools (swiftc, codesign) and cargo. Developer ID signing and notarization are not done here.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ime=$(cd "$here/.." && pwd)
: "${TWP_ASSETS:?set TWP_ASSETS to the directory of the model assets}"
[ -f "$TWP_ASSETS/model.bin" ] || { echo "TWP_ASSETS has no model.bin: $TWP_ASSETS" >&2; exit 1; }
export MACOSX_DEPLOYMENT_TARGET=14.0

(cd "$ime" && cargo build --release --locked -p twime-shell -p twime-engined)

app="$here/build/TwPotato.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/assets"
swiftc -O -target arm64-apple-macos14.0 -I "$here/CTwime" "$here"/Sources/*.swift "$ime/target/release/libtwime_shell.a" \
    -framework InputMethodKit -framework Cocoa -o "$app/Contents/MacOS/TwPotato"
cp "$ime/target/release/twime-engined" "$app/Contents/MacOS/"
cp "$here/Info.plist" "$app/Contents/"
# Only plain files, one level: what the engine reads.
for f in "$TWP_ASSETS"/*; do
    [ -f "$f" ] && cp "$f" "$app/Contents/Resources/assets/"
done
for f in LICENSE THIRD-PARTY-NOTICES.md MODEL_CARD.md; do
    cp "$ime/../$f" "$app/Contents/Resources/"
done
# Inside out: the engine first, then the bundle.
codesign --force --sign - "$app/Contents/MacOS/twime-engined"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
echo "built $app"
