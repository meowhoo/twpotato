"""The smallest GTK 3 program with a text field, for t_gtk.py: it writes what its text field experiences to a log.

    gtk_box.py LOG      READY <X window id> | PREEDIT <text> | INSERT <text put into the field, when not empty> | LINE <field at Enter>

Enter while nothing is being composed records the field's text as a LINE and empties the field."""
import sys
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("GdkX11", "3.0")
from gi.repository import GdkX11, GLib, Gtk  # noqa: F401 (GdkX11 gives the window its get_xid)

log = open(sys.argv[1], "a", buffering=1, encoding="utf-8")
win = Gtk.Window(title="twpotato-gtk-box")
entry = Gtk.Entry()
win.add(entry)


def on_activate(e):
    log.write("LINE\t" + e.get_text() + "\n")
    e.set_text("")


entry.connect("activate", on_activate)
entry.connect("preedit-changed", lambda _e, text: log.write("PREEDIT\t" + text + "\n"))
entry.connect("insert-text", lambda _e, text, _length, _position: text and log.write("INSERT\t" + text + "\n"))
win.connect("destroy", Gtk.main_quit)
win.show_all()
entry.grab_focus()
GLib.idle_add(lambda: log.write(f"READY\t{win.get_window().get_xid()}\n") and False)
Gtk.main()
