#!/usr/bin/env python3
"""A small GTK window for Live (Linux)'s computer-use checks (docs/parity/ledger.toml):
one label, one entry and one button, titled so `computer elements` finds it. It exists so
the check has an app with an accessibility tree on a bare Xvfb, where no desktop app is
installed. NO_AT_BRIDGE comes off before GTK starts: headless.sh sets it for Trek's own
web views, and this window must be on the accessibility bus for `elements` to read it.
"""

import os

os.environ.pop("NO_AT_BRIDGE", None)

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk

window = Gtk.Window(title="Trek Computer Check")
window.set_default_size(420, 160)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
window.add(box)

label = Gtk.Label(label="Trek computer use check")
entry = Gtk.Entry()
entry.set_placeholder_text("type here")
button = Gtk.Button(label="Check me")
for widget in (label, entry, button):
    box.pack_start(widget, False, False, 0)

window.connect("destroy", Gtk.main_quit)
window.show_all()
window.present()
# No window manager on Xvfb: take the input focus itself, so `elements` (which reads the
# window in front) finds this one and not Trek's.
window.get_window().focus(0)
Gtk.main()
