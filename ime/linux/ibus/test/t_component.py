"""The component files cmake writes, for a build directory and an install prefix whose names contain a space, an
ampersand, a quote and angle brackets: both must be well-formed XML, and their <exec>, split as a command line the
way ibus-daemon does it (GLib's shell rules), must be exactly the executable's path and `--ibus`.
Only cmake's configure step runs; nothing is built."""
import os, subprocess, xml.etree.ElementTree as ET

from gi.repository import GLib

HERE = os.path.dirname(os.path.abspath(__file__))


def run():
    stage = os.environ["TWFX_STAGE"]
    build = os.path.join(stage, "build dir & 'x' <y>")
    prefix = os.path.join(stage, "prefix & 'p' <q>")
    r = subprocess.run(["cmake", "-S", os.path.join(HERE, ".."), "-B", build, f"-DCMAKE_INSTALL_PREFIX={prefix}",
                        f"-DTWIME_SHELL_LIB={os.environ['TWFX_SHELL_LIB']}"], capture_output=True, text=True)
    if r.returncode != 0:
        print(r.stdout[-2000:], r.stderr[-2000:])
        return False
    ok = True
    for xml, exe in ((os.path.join(build, "twpotato.xml"), os.path.join(build, "ibus-engine-twpotato")),
                     (os.path.join(build, "install", "twpotato.xml"), os.path.join(prefix, "lib", "twpotato", "ibus-engine-twpotato"))):
        try:
            command = ET.parse(xml).getroot().findtext("exec")
            _, argv = GLib.shell_parse_argv(command)
        except (ET.ParseError, GLib.Error) as e:
            print(f"component: {xml}: {e}")
            ok = False
            continue
        if argv != [exe, "--ibus"]:
            print(f"component: {xml}: <exec> splits into {argv}, expected {[exe, '--ibus']}")
            ok = False
    print(f"component: both component files are well-formed and name the executable as one argument: {'yes' if ok else 'NO'}")
    return ok
