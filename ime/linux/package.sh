#!/bin/bash
# Builds the three Ubuntu packages of TwPotato from this source tree (LINUX-plan 6.2), on the Ubuntu release
# they are for (24.04 or 26.04, amd64). One run makes one set; the three always carry the same version.
#
#     TWP_ASSETS=<directory of the model assets> ime/linux/package.sh
#
#   twpotato          /usr/lib/twpotato/twime-engined, /usr/share/twpotato/assets/*, licenses and notices
#   fcitx5-twpotato   the Fcitx5 addon twpotato.so and its two configuration files; depends on twpotato and fcitx5
#   ibus-twpotato     /usr/lib/twpotato/ibus-engine-twpotato and the IBus component file; depends on twpotato and ibus
#
# Version: <upstream>-1~ubuntu<release>.<revision>, e.g. 0.1.0-1~ubuntu24.04.1. <upstream> is the version of the
# twime-engined crate; <revision> is TWP_REVISION.
#
# Environment:
#   TWP_ASSETS        the model assets to ship: a flat directory with model.bin, data.txt, ... (required). Only files
#                     of this directory get into the package; nothing else of the machine does.
#   TWP_REVISION      packaging revision, a positive number (default 1). Raise it to repackage the same upstream version.
#   TWP_OUT           where the .deb files and the manifest go (default $TWP_BUILD/deb)
#   TWP_MAINTAINER    the Maintainer field, "name <address>". Default: a placeholder address (maintainer@invalid),
#                     good for test builds only.
#   TWP_RELEASE       1: packages to publish. Refuses the placeholder Maintainer address and a source tree with
#                     uncommitted changes to the files that go into the packages.
#   CARGO_TARGET_DIR, TWP_BUILD, JOBS   as in build.sh; TWP_BUILD defaults to $CARGO_TARGET_DIR/linux-pkg here
#   SOURCE_DATE_EPOCH the time stamp of the files in the packages (default: now)
# Needs what build.sh needs, plus dpkg-dev and binutils. No root, no debhelper. The packages have no maintainer
# scripts: installing, upgrading and removing only places and removes files (LINUX-plan.md 4.3).
#
# License checks that stop the build: the programs may be linked dynamically only to the system libraries listed below
# (Fcitx5, IBus, GLib, the C / C++ runtime); no shared library but our own addon is packaged; every Rust crate in the
# programs is named in THIRD-PARTY-NOTICES.md. The LGPL text shipped next to the two front ends is the build system's
# /usr/share/common-licenses/LGPL-2.1.
set -eu
umask 022
here=$(cd "$(dirname "$0")" && pwd)
ime=$(cd "$here/.." && pwd)
repo=$(cd "$ime/.." && pwd)
die() { echo "package.sh: $*" >&2; exit 1; }

# Absolute paths: parts of this script work from other directories.
target=$(readlink -m "${CARGO_TARGET_DIR:-$ime/target}")
build=$(readlink -m "${TWP_BUILD:-$target/linux-pkg}")
out=$(readlink -m "${TWP_OUT:-$build/deb}")
rev=${TWP_REVISION:-1}
maintainer=${TWP_MAINTAINER:-meowhoo <maintainer@invalid>}
release=${TWP_RELEASE:-0}
homepage=https://github.com/meowhoo/twpotato
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(date +%s)}

# ---- what is being built, and where ----
[ "$(id -u)" != 0 ] || die "do not run as root"
[[ $maintainer =~ ^[^\<\>]+\ \<[^@\<\>\ ]+@[^@\<\>\ ]+\>$ ]] || die "TWP_MAINTAINER must be 'name <address>': '$maintainer'"
case $release in
    0) ;;
    1)
        [[ $maintainer != *@invalid\> ]] || die "TWP_RELEASE=1: give the real Maintainer address in TWP_MAINTAINER"
        if git -C "$repo" rev-parse --git-dir > /dev/null 2>&1; then
            [ -z "$(git -C "$repo" status --porcelain -- ime rust-toolchain.toml LICENSE THIRD-PARTY-NOTICES.md third_party emoji/LICENSE-UNICODE-3.0.txt)" ] ||
                die "TWP_RELEASE=1: the source tree has uncommitted changes"
        else
            die "TWP_RELEASE=1: not a git checkout; build release packages from the repository"
        fi
        ;;
    *) die "TWP_RELEASE must be 0 or 1: '$release'" ;;
esac
[[ $rev =~ ^[1-9][0-9]*$ ]] || die "TWP_REVISION must be a positive number: '$rev'"
. /etc/os-release
case "$ID:$VERSION_ID" in
    ubuntu:24.04 | ubuntu:26.04) ;;
    *) die "packages are built on Ubuntu 24.04 or 26.04 only (this is $ID $VERSION_ID)" ;;
esac
arch=$(dpkg --print-architecture)
[ "$arch" = amd64 ] || die "amd64 only (this is $arch)"
multiarch=$(dpkg-architecture -qDEB_HOST_MULTIARCH)
upstream=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' "$ime/crates/twime-engined/Cargo.toml" | head -n 1)
[ -n "$upstream" ] || die "no version in crates/twime-engined/Cargo.toml"
for f in fcitx5 ibus; do  # the addon file and the component file carry the cmake project version
    grep -q "^project(.* VERSION $upstream " "$here/$f/CMakeLists.txt" || die "$f/CMakeLists.txt: project VERSION is not $upstream"
done
version="$upstream-1~ubuntu$VERSION_ID.$rev"

assets=${TWP_ASSETS:?set TWP_ASSETS to the directory of the model assets}
assets=$(readlink -f "$assets")
[ -f "$assets/model.bin" ] && [ -f "$assets/data.txt" ] || die "no model.bin / data.txt in $assets"
[ -z "$(find "$assets" -mindepth 1 ! -type f -print -quit)" ] || die "$assets must hold plain files only (no directories, no links)"
lgpl=/usr/share/common-licenses/LGPL-2.1
grep -q "GNU LESSER GENERAL PUBLIC LICENSE" "$lgpl" && grep -q "Version 2.1, February 1999" "$lgpl" || die "$lgpl is not the LGPL 2.1 text"
docs=("$repo/LICENSE" "$repo/THIRD-PARTY-NOTICES.md" "$repo/third_party/mcbopomofo/LICENSE-McBopomofo.txt"
      "$repo/third_party/mcbopomofo/COPYING-libtabe.txt" "$repo/emoji/LICENSE-UNICODE-3.0.txt")
for f in "${docs[@]}"; do [ -f "$f" ] || die "missing: $f"; done

# ---- build ----
TWP_BUILD=$build TWP_PREFIX=/usr CARGO_TARGET_DIR=$target "$here/build.sh"

# Every crate linked into the Linux programs must be named in the notices file.
(cd "$ime" && cargo tree --locked -p twime-engined -p twime-shell -e normal --prefix none --format '{p}') |
    awk 'NF >= 2 && $3 !~ /^\(\// { print $1, substr($2, 2) }' | sort -u > "$build/crates.txt"   # not the workspace's own
[ -s "$build/crates.txt" ] || die "cargo tree listed no crate"
while read -r name ver; do
    grep -q -F "\`$name $ver\`" "$repo/THIRD-PARTY-NOTICES.md" || die "crate $name $ver is not in THIRD-PARTY-NOTICES.md"
done < "$build/crates.txt"

# ---- the three trees ----
stage=$build/stage
rm -rf "$stage"
mkdir -p "$stage" "$out"
t=$stage/twpotato f=$stage/fcitx5-twpotato i=$stage/ibus-twpotato

install -D -m 755 "$target/release/twime-engined" "$t/usr/lib/twpotato/twime-engined"
mkdir -p "$t/usr/share/twpotato/assets"
install -m 644 "$assets"/* "$t/usr/share/twpotato/assets/"
DESTDIR=$f cmake --install "$build/fcitx5" > /dev/null
DESTDIR=$i cmake --install "$build/ibus" > /dev/null

# Exactly these files, nothing else (a changed install rule or an unexpected prefix stops here).
addon=usr/lib/$multiarch/fcitx5/twpotato.so
expect() {  # <tree> <file>...
    local tree=$1; shift
    diff <(cd "$tree" && find . -type f ! -path './usr/share/twpotato/assets/*' | sed 's|^\./||' | sort) <(printf '%s\n' "$@" | sort) ||
        die "unexpected set of files in $tree (left: found, right: expected)"
}
expect "$t" usr/lib/twpotato/twime-engined
expect "$f" "$addon" usr/share/fcitx5/addon/twpotato.conf usr/share/fcitx5/inputmethod/twpotato.conf
expect "$i" usr/lib/twpotato/ibus-engine-twpotato usr/share/ibus/component/twpotato.xml
grep -q -F "<exec>&apos;/usr/lib/twpotato/ibus-engine-twpotato&apos; --ibus</exec>" "$i/usr/share/ibus/component/twpotato.xml" ||
    die "the component file does not name /usr/lib/twpotato/ibus-engine-twpotato"
grep -q -x "Library=twpotato" "$f/usr/share/fcitx5/addon/twpotato.conf" || die "the addon file does not name the library twpotato"

strip "$t/usr/lib/twpotato/twime-engined" "$i/usr/lib/twpotato/ibus-engine-twpotato"
strip --strip-unneeded "$f/$addon"
chmod 755 "$t/usr/lib/twpotato/twime-engined" "$i/usr/lib/twpotato/ibus-engine-twpotato"
chmod 644 "$f/$addon"

# Dynamic linking: only these system libraries.
base='libc\.so\.6|libm\.so\.6|libgcc_s\.so\.1|ld-linux-x86-64\.so\.2'
needed() {  # <file> <allowed libraries, a regular expression>
    local n
    n=$(readelf -d "$1" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
    [ -n "$n" ] || die "$1: no dynamic section"
    if echo "$n" | grep -v -E -x "$base|$2"; then die "$1 is linked to the libraries above, which are not on its list"; fi
    echo "$1: $(echo $n)" | sed "s|^$stage/||"
}
{
    needed "$t/usr/lib/twpotato/twime-engined" 'none'
    needed "$f/$addon" 'libFcitx5(Core|Utils|Config)\.so\.[0-9]+|libstdc\+\+\.so\.6'
    needed "$i/usr/lib/twpotato/ibus-engine-twpotato" 'libibus-1\.0\.so\.5|lib(glib|gobject|gio)-2\.0\.so\.0'
} > "$build/needed.txt"

# ---- documents: copyright (with the notices each package needs), license texts, changelog ----
stamp=$(date -u -R -d "@$SOURCE_DATE_EPOCH")
for pkg in twpotato fcitx5-twpotato ibus-twpotato; do
    doc=$stage/$pkg/usr/share/doc/$pkg
    mkdir -p "$doc"
    sed -e "s|@HOMEPAGE@|$homepage|" -e "/^@LICENSE@\$/{r $repo/LICENSE" -e "d}" "$here/packaging/copyright-$pkg" > "$doc/copyright"
    ! grep -q '@[A-Z]*@' "$doc/copyright" || die "placeholder left in $doc/copyright"
    printf 'twpotato (%s) %s; urgency=medium\n\n  * TwPotato %s for Ubuntu %s, packaging revision %s.\n\n -- %s  %s\n' \
        "$version" "$VERSION_CODENAME" "$upstream" "$VERSION_ID" "$rev" "$maintainer" "$stamp" | gzip -9n > "$doc/changelog.Debian.gz"
done
install -m 644 "${docs[@]:1}" "$t/usr/share/doc/twpotato/"
install -m 644 "$lgpl" "$f/usr/share/doc/fcitx5-twpotato/LGPL-2.1.txt"
install -m 644 "$lgpl" "$i/usr/share/doc/ibus-twpotato/LGPL-2.1.txt"

# No shared library but our own addon is packaged.
other=$(cd "$stage" && find . -name '*.so' -o -name '*.so.*' -o -name '*.a' | grep -v -x "./fcitx5-twpotato/$addon" || true)
[ -z "$other" ] || die "libraries that must not be packaged: $other"

# ---- control files and the packages ----
# dpkg-shlibdeps wants a source tree around it: an empty one.
mkdir -p "$build/shlibs/debian"
printf 'Source: twpotato\n\nPackage: twpotato\nArchitecture: %s\n' "$arch" > "$build/shlibs/debian/control"
shlibs() {  # <binary> -> the packages of the libraries it is linked to, with the versions it needs
    local d
    (cd "$build/shlibs" && dpkg-shlibdeps -O -e"$1") > "$build/shlibs/out" 2> "$build/shlibs/log" ||
        { cat "$build/shlibs/log" >&2; die "dpkg-shlibdeps failed for $1"; }
    d=$(sed -n 's/^shlibs:Depends=//p' "$build/shlibs/out")
    [ -n "$d" ] || { cat "$build/shlibs/log" >&2; die "dpkg-shlibdeps found nothing for $1"; }
    echo "$d"
}
# Each on a line of its own: a failure inside $( ) ends the script only when the assignment is the whole command.
deps_engine=$(shlibs "$t/usr/lib/twpotato/twime-engined")
deps_addon=$(shlibs "$f/$addon")
deps_ibus=$(shlibs "$i/usr/lib/twpotato/ibus-engine-twpotato")
pack() {  # <package> <depends> <synopsis> <description line>...
    local pkg=$1 depends=$2 synopsis=$3 tree=$stage/$1
    shift 3
    find "$tree" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
    mkdir -p "$tree/DEBIAN"
    {
        echo "Package: $pkg"
        [ "$pkg" = twpotato ] || echo "Source: twpotato"
        echo "Version: $version"
        echo "Architecture: $arch"
        echo "Maintainer: $maintainer"
        echo "Installed-Size: $(du -sk --exclude=DEBIAN "$tree" | cut -f1)"
        echo "Depends: $depends"
        echo "Section: utils"
        echo "Priority: optional"
        echo "Homepage: $homepage"
        echo "Description: $synopsis"
        printf ' %s\n' "$@"
    } > "$tree/DEBIAN/control"
    (cd "$tree" && find . -type f ! -path './DEBIAN/*' -printf '%P\0' | LC_ALL=C sort -z | xargs -0 md5sum) > "$tree/DEBIAN/md5sums"
    chmod 644 "$tree/DEBIAN/control" "$tree/DEBIAN/md5sums"
    dpkg-deb --root-owner-group -Zxz --build "$tree" "$out/${pkg}_${version}_${arch}.deb" > /dev/null
}
pack twpotato "$deps_engine" \
    "bopomofo input method TwPotato - engine and model" \
    "TwPotato (地瓜輸入法) is a bopomofo (zhuyin) input method for Traditional Chinese" \
    "that converts whole sentences with a neural model, on this machine only." \
    "." \
    "This package has the engine and the model. To type with it, install" \
    "ibus-twpotato (IBus, the default of the Ubuntu desktop) or fcitx5-twpotato."
pack fcitx5-twpotato "twpotato (= $version), fcitx5, $deps_addon" \
    "bopomofo input method TwPotato - Fcitx5 addon" \
    "TwPotato (地瓜輸入法) is a bopomofo (zhuyin) input method for Traditional Chinese." \
    "This package makes it available in Fcitx5."
pack ibus-twpotato "twpotato (= $version), ibus, $deps_ibus" \
    "bopomofo input method TwPotato - IBus engine" \
    "TwPotato (地瓜輸入法) is a bopomofo (zhuyin) input method for Traditional Chinese." \
    "This package makes it available in IBus (on the GNOME desktop: Settings,"     "Keyboard, Input Sources)."

# ---- manifest: what was made, from what ----
manifest=$out/twpotato_${version}_${arch}.manifest.txt
{
    echo "TwPotato $version, built on $PRETTY_NAME ($(uname -r)) at $stamp"
    echo "rustc: $(cd "$ime" && rustc --version); gcc: $(gcc -dumpfullversion); cmake: $(cmake --version | head -n 1 | cut -d' ' -f3)"
    echo "fcitx5: $(pkg-config --modversion Fcitx5Core); ibus: $(pkg-config --modversion ibus-1.0); glib: $(pkg-config --modversion glib-2.0)"
    echo; echo "== dynamic libraries (NEEDED) =="; cat "$build/needed.txt"
    echo; echo "== assets (sha256) =="; (cd "$assets" && sha256sum -- *)
    for pkg in twpotato fcitx5-twpotato ibus-twpotato; do
        deb=$out/${pkg}_${version}_${arch}.deb
        echo; echo "== $(basename "$deb"): $(stat -c %s "$deb") bytes, sha256 $(sha256sum "$deb" | cut -d' ' -f1) =="
        dpkg-deb -f "$deb"
        dpkg-deb -c "$deb" | awk '{ print $1, $2, $3, $6 }'
    done
} > "$manifest"
echo "packages: $out/{twpotato,fcitx5-twpotato,ibus-twpotato}_${version}_${arch}.deb"
echo "manifest: $manifest"
