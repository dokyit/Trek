# dmgbuild settings for Trek's disk image (script/dmg.sh passes the -D values).
#
# The window: Trek.app on the left, an Applications link on the right, over
# assets/dmg/background.png (and its @2x, which dmgbuild finds and joins into one HiDPI image).
# The icon positions are the centres the background's arrow is drawn between: change
# assets/dmg/background.svg with them.
import os.path

app = defines["app"]  # noqa: F821 (dmgbuild provides `defines`)
background = defines["background"]  # noqa: F821
icon = defines["icon"]  # noqa: F821, the volume icon: Trek's own

files = [app]
symlinks = {"Applications": "/Applications"}

# ULFO (LZFSE): smaller than UDZO and opens on every macOS Trek runs on (10.11 and later).
format = "ULFO"
filesystem = "HFS+"
# Room for the app plus the filesystem; dmgbuild sizes the image from the files when unset.
size = None

# The window's frame, title bar included (32 pt on macOS 26 and 27, less before): its content
# is 640 x 400 pt there, and the background is 420 pt tall, so an older, shorter title bar still
# shows background, not a blank strip.
window_rect = ((200, 140), (640, 432))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
include_icon_view_settings = True
include_list_view_settings = False

arrange_by = None
grid_offset = (0, 0)
grid_spacing = 100
scroll_position = (0, 0)
label_pos = "bottom"
text_size = 13
icon_size = 128

icon_locations = {
    os.path.basename(app): (160, 190),
    "Applications": (480, 190),
}
hide_extension = [os.path.basename(app)]
