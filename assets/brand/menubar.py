# Menu-bar template glyphs (black + alpha, macOS tints them). 18pt canvas = 36px @2x.
# Simplified 3-tier switchback narrowing upward, ending in the summit beacon.
def glyph(state):
    stroke = 'stroke="#000" fill="none" stroke-linecap="round" stroke-linejoin="round"'
    trail = (f'<path d="M5 30.5 H27 Q31 30.5 31 27 Q31 23.5 27 23.5 H11 Q7.5 23.5 7.5 20.5 Q7.5 17.5 11 17.5 '
             f'H22 Q25 17.5 25 15 Q25 12.5 22 12.5 H18 V10.5" {stroke} stroke-width="3"/>')
    if state == "idle":
        beacon = f'<circle cx="18" cy="6" r="2.6" {stroke} stroke-width="2"/>'
    else:
        beacon = '<circle cx="18" cy="6" r="3.4" fill="#000"/>'
    badge = '<circle cx="31.5" cy="5" r="3.2" fill="#000"/>' if state == "attention" else ""
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 36 36" width="36" height="36">{trail}{beacon}{badge}</svg>'
for s in ("idle", "working", "attention"):
    open(f"menubar-{s}.svg", "w").write(glyph(s))
