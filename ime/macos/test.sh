#!/bin/sh
# The tests of the macOS shell (MAC-M2-plan 4.1): the controller in a test program with a recording client,
# the real shell library and the real engine. No window is opened and nothing of the user is touched: every group runs
# with HOME in a stage of its own under /tmp.
#   TWP_ASSETS=<assets> ime/macos/test.sh [GROUP ...]     groups: unit modifiers functional faults noassets userlex full
#   full needs TWP_CASES=<cases file in the format of ime/xcmp> (not in the repository)
#   TWP_OUT=<directory> keeps the results there (default: build/test-out)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ime=$(cd "$here/.." && pwd)
: "${TWP_ASSETS:?set TWP_ASSETS to the directory of the model assets}"
assets=$(cd "$TWP_ASSETS" && pwd)
out=${TWP_OUT:-$here/build/test-out}
mkdir -p "$out"
export MACOSX_DEPLOYMENT_TARGET=14.0

(cd "$ime" && cargo build --release --locked -p twime-shell -p twime-engined)
(cd "$ime/xcmp" && cargo build --release --locked)
engine="$ime/target/release/twime-engined"
xcmp="$ime/xcmp/target/release/xcmp"
# The reference and the shell must start the same engine with the same defaults next to it.
for f in user-data.on legacy-defaults.toml; do
    [ ! -e "$ime/target/release/$f" ] || { echo "$ime/target/release/$f would change the engine's defaults" >&2; exit 1; }
done
mkdir -p "$here/build"
swiftc -O -target arm64-apple-macos14.0 -I "$here/CTwime" \
    "$here"/Sources/KeyMap.swift "$here"/Sources/ModifierTracker.swift "$here"/Sources/Apply.swift \
    "$here"/Sources/CandidatePanel.swift "$here"/Sources/Controller.swift "$here"/Tests/*.swift \
    "$ime/target/release/libtwime_shell.a" -framework InputMethodKit -framework Cocoa -o "$here/build/twpotato-tests"

stages=""
# True while this process is an engine of stage $2 (ww: the command line is never cut short).
engine_of() { [ "$1" -gt 0 ] 2>/dev/null && ps -ww -p "$1" -o command= 2>/dev/null | grep -qF "$2/T.app"; }
cleanup() {
    for s in $stages; do
        # Engines of this stage, by the process ids its wrapper noted (stopped ones too). The stage, and with it the
        # record of those ids, goes only when none of them is left.
        left=0
        if [ -f "$s/engine-starts" ]; then
            while read -r pid; do
                if engine_of "$pid" "$s"; then
                    kill -CONT "$pid" 2>/dev/null || true
                    kill -KILL "$pid" 2>/dev/null || true
                    n=0
                    while engine_of "$pid" "$s" && [ $n -lt 100 ]; do sleep 0.05; n=$((n + 1)); done
                    if engine_of "$pid" "$s"; then left=1; echo "engine $pid of $s does not end: the stage is kept" >&2; fi
                fi
            done < "$s/engine-starts"
        fi
        [ $left -ne 0 ] || rm -rf "$s"
    done
}
trap cleanup EXIT
# Interrupted or terminated: leave through the EXIT trap (a signal alone would skip it).
trap 'exit 130' INT
trap 'exit 143' TERM

# A stage: the test program in the shape of the bundle, a wrapper for the engine that notes every start, an empty home.
# $1: "assets" or "noassets"; $2: a user lexicon to install, or nothing.
new_stage() {
    stage=$(mktemp -d /tmp/twpm.XXXXXX)
    stages="$stages $stage"
    mkdir -p "$stage/T.app/Contents/MacOS" "$stage/T.app/Contents/Resources" "$stage/home/Library/Caches" \
        "$stage/home/Library/Application Support/TwPotato"
    cp "$here/build/twpotato-tests" "$stage/T.app/Contents/MacOS/"
    [ "$1" = noassets ] || ln -s "$assets" "$stage/T.app/Contents/Resources/assets"
    cat > "$stage/T.app/Contents/MacOS/twime-engined" <<WRAP
#!/bin/sh
echo \$\$ >> "$stage/engine-starts"
exec "$engine" "\$@"
WRAP
    chmod +x "$stage/T.app/Contents/MacOS/twime-engined"
    if [ -n "${2:-}" ]; then cp "$2" "$stage/home/Library/Application Support/TwPotato/userlex.tsv"
    else : > "$stage/home/Library/Application Support/TwPotato/userlex.tsv"; fi
}

run() {
    env -i PATH="$PATH" HOME="$stage/home" TWP_STAGE="$stage" "$stage/T.app/Contents/MacOS/twpotato-tests" "$@"
}

# What the engine itself answers to the cases: xcmp, protocol 5, user data on, every reply (XCMP_STEPS).
# $1: cases; $2: directory with userlex.tsv; $3: prefix of the reference files
reference() {
    scratch=$(mktemp -d /tmp/twpm.XXXXXX)
    stages="$stages $scratch"
    XCMP_STEPS=1 "$xcmp" "$engine" "$assets" "$1" "$3" "$scratch/x" 5 "$2" > /dev/null 2> "$3.steps"
}

failed=""
for group in ${@:-unit modifiers functional faults noassets userlex}; do
    echo "== $group"
    case $group in
    unit) new_stage assets; run unit || failed="$failed $group" ;;
    modifiers | functional | faults) new_stage assets; run "$group" || failed="$failed $group" ;;
    noassets) new_stage noassets; run noassets || failed="$failed $group" ;;
    userlex)
        reference "$ime/xcmp/cases-userlex.tsv" "$ime/xcmp/userlex" "$out/ref-userlex"
        new_stage assets "$ime/xcmp/userlex/userlex.tsv"
        run cases "$ime/xcmp/cases-userlex.tsv" "$out/ref-userlex.out.tsv" "$out/ref-userlex.steps" "$out/cases-userlex" || failed="$failed $group" ;;
    full)
        : "${TWP_CASES:?set TWP_CASES to the cases file}"
        empty=$(mktemp -d /tmp/twpm.XXXXXX); stages="$stages $empty"; : > "$empty/userlex.tsv"
        reference "$TWP_CASES" "$empty" "$out/ref-full"
        new_stage assets
        run cases "$TWP_CASES" "$out/ref-full.out.tsv" "$out/ref-full.steps" "$out/cases-full" || failed="$failed $group" ;;
    *) echo "unknown group $group" >&2; exit 2 ;;
    esac
done
[ -z "$failed" ] || { echo "FAILED:$failed"; exit 1; }
echo "all groups passed"
