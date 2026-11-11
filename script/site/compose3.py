"""The site's master pictures of one window, from its captures.

compose3.py RAW_DIR OUT_DIR THEME
  in   RAW_DIR/real-THEME.png        screencapture -l, glass off: chrome, corners and shadow as macOS drew them
       RAW_DIR/realglass-THEME.png   the same with glass on (its corner shape and button places)
       RAW_DIR/glass-THEME-rgba.png  the glass frame with its real alpha (unglass.py)
  out  OUT_DIR/plain-THEME.png       the whole window with its shadow, on transparency
       OUT_DIR/glass-THEME.png       the glass frame: see-through sidebar and title bar, corner shape, no shadow

The one retouch: the three title-bar buttons. The test window is never the key window, so macOS
draws them grey; they are painted here in macOS's colours, exactly where and as large as captured.
"""
import sys
from PIL import Image, ImageChops, ImageDraw

raw, out, theme = sys.argv[1:4]
LIGHTS = [("#FF5F57", "#E0443E"), ("#FEBC2E", "#DEA123"), ("#28C840", "#1AAB29")]  # fill, rim


def window_box(im):
    return im.getchannel("A").point(lambda v: 255 if v == 255 else 0).getbbox()


# Where macOS draws the three buttons in this window, in 2x pixels from its top-left corner:
# centre x, centre y, diameter. Measured on the capture (their faint rims run x 18-46, 64-92 and
# 110-138, y 18-46): 14-point circles, 23 points apart, the same at every window size.
BUTTONS = [(32, 32, 28), (78, 32, 28), (124, 32, 28)]


def buttons_are_there(win):
    """The capture still has a rim where each button is expected (a top edge unlike the bar)."""
    px = win.convert("RGB").load()
    bar = sum(px[6, 32]) / 3
    return all(any(abs(sum(px[cx, cy - d // 2 + k]) / 3 - bar) > 3 for k in (-1, 0, 1, 2)) for cx, cy, d in BUTTONS)


def paint(im, buttons, dx=0, dy=0):
    k = 4
    for (cx, cy, d), (fill, rim) in zip(buttons, LIGHTS):
        size = int(d + 4)
        layer = Image.new("RGBA", (size * k, size * k), (0, 0, 0, 0))
        draw = ImageDraw.Draw(layer)
        m = (size - d) / 2 * k
        draw.ellipse((m, m, size * k - m, size * k - m), fill=rim)
        draw.ellipse((m + k, m + k, size * k - m - k, size * k - m - k), fill=fill)
        layer = layer.resize((size, size), Image.LANCZOS)
        im.alpha_composite(layer, (round(dx + cx - size / 2), round(dy + cy - size / 2)))


# --- plain: the capture as it is, buttons painted --------------------------------------------
real = Image.open(f"{raw}/real-{theme}.png").convert("RGBA")
box = window_box(real)
buttons = BUTTONS
if not buttons_are_there(real.crop(box)):
    print(f"{theme}: warning: no button rims where they were measured; check the title bar by eye")
paint(real, buttons, box[0], box[1])
real.save(f"{out}/plain-{theme}.png")

# --- glass: real alpha, the window's corner shape, buttons painted -----------------------------
import os
if not os.path.exists(f"{raw}/realglass-{theme}.png"):
    print(f"{theme}: plain {real.size} (window {box[2] - box[0]}x{box[3] - box[1]} at {box[0]},{box[1]}), no glass frame taken")
    sys.exit(0)
greal = Image.open(f"{raw}/realglass-{theme}.png").convert("RGBA")
gbox = window_box(greal)
ui = Image.open(f"{raw}/glass-{theme}-rgba.png").convert("RGBA")
shape = greal.getchannel("A").crop(gbox)
if shape.size != ui.size:
    sys.exit(f"{theme}: glass frame {ui.size} and capture {shape.size} differ")
ui.putalpha(ImageChops.multiply(ui.getchannel("A"), shape))
paint(ui, buttons)
ui.save(f"{out}/glass-{theme}.png")
print(f"{theme}: plain {real.size} (window {box[2] - box[0]}x{box[3] - box[1]} at {box[0]},{box[1]}), glass {ui.size}, "
      f"buttons at {[(round(x), round(y), d) for x, y, d in buttons]}")
