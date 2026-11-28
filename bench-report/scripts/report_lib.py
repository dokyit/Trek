"""Small helpers for build_report.py: escaping, inline SVG bar charts, tables. No dependencies."""
import html

TREK = "Trek"


def esc(s):
    return html.escape(str(s), quote=True)


def fmt(v, unit="", digits=0):
    if v is None:
        return "–"
    if isinstance(v, (int, float)):
        s = f"{v:,.{digits}f}"
    else:
        s = str(v)
    return f"{s}{unit}"


def hbar(rows, title, unit="", digits=0, note="", lower_is_better=True, max_value=None, width=720):
    """Horizontal bars, one series: Trek in the accent, the rest neutral. rows: [(label, value, extra_label)].
    A bar per row, sorted best first; values labelled at the bar's end (selective, one per bar is fine
    for a ranked list of <= 16)."""
    rows = [r for r in rows if r[1] is not None]
    rows.sort(key=lambda r: r[1], reverse=not lower_is_better)
    if not rows:
        return ""
    row_h, top, left, right = 26, 8, 170, 120
    h = top + row_h * len(rows) + 8
    vmax = max_value or max(r[1] for r in rows) * 1.02
    plot = width - left - right
    out = [f'<figure class="chart"><figcaption class="chart-title">{esc(title)}</figcaption>',
           f'<svg viewBox="0 0 {width} {h}" role="img" aria-label="{esc(title)}">']
    for i, r in enumerate(rows):
        label, v = r[0], r[1]
        extra = r[2] if len(r) > 2 else ""
        y = top + i * row_h
        w = max(2, plot * min(v, vmax) / vmax)
        cls = "bar trek" if label.startswith(TREK) else "bar"
        out.append(f'<g><title>{esc(label)}: {esc(fmt(v, unit, digits))}{(" · " + esc(extra)) if extra else ""}</title>')
        out.append(f'<text class="lbl{" trek-l" if label.startswith(TREK) else ""}" x="{left - 10}" y="{y + 17}" text-anchor="end">{esc(label)}</text>')
        out.append(f'<rect class="{cls}" x="{left}" y="{y + 5}" width="{w:.1f}" height="16" rx="4"/>')
        clipped = " ›" if v > vmax else ""
        out.append(f'<text class="val" x="{left + w + 6:.1f}" y="{y + 17}">{esc(fmt(v, unit, digits))}{clipped}{(" " + esc(extra)) if extra else ""}</text></g>')
    out.append("</svg>")
    if note:
        out.append(f'<p class="chart-note">{note}</p>')
    out.append("</figure>")
    return "\n".join(out)


def table(headers, rows, cls="data"):
    out = [f'<div class="table-wrap"><table class="{cls}"><thead><tr>']
    out += [f"<th>{h}</th>" for h in headers]
    out.append("</tr></thead><tbody>")
    for r in rows:
        tr_cls = ""
        if isinstance(r, dict):
            tr_cls, r = r.get("cls", ""), r["cells"]
        out.append(f'<tr class="{tr_cls}">' + "".join(f"<td>{c}</td>" for c in r) + "</tr>")
    out.append("</tbody></table></div>")
    return "\n".join(out)


CSS = """
:root{--bg:#f6f5f1;--surface:#fffefb;--ink:#1b1c1a;--ink-2:#55574f;--ink-3:#6f7168;--line:#dcdad2;--accent:#c2410c;--accent-soft:#fbe6da;--neutral:#a8a59a;--good:#1f7a4a;--good-soft:#dff1e6;--bad:#b42318;--bad-soft:#fbe3e0;--mid:#8a6a00;--mid-soft:#f6edcf;
--serif:"Newsreader",Georgia,serif;--sans:"Inter Tight","Inter",system-ui,-apple-system,sans-serif;--mono:"JetBrains Mono",ui-monospace,Menlo,monospace;color-scheme:light}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--bg:#141512;--surface:#1c1d1a;--ink:#ecebe5;--ink-2:#b4b2a8;--ink-3:#97958b;--line:#33342f;--accent:#f0864f;--accent-soft:#3a2418;--neutral:#6d6b63;--good:#5cc08a;--good-soft:#173324;--bad:#f07a6e;--bad-soft:#3a1d1a;--mid:#e0bb4a;--mid-soft:#352c12;color-scheme:dark}}
:root[data-theme="dark"]{--bg:#141512;--surface:#1c1d1a;--ink:#ecebe5;--ink-2:#b4b2a8;--ink-3:#97958b;--line:#33342f;--accent:#f0864f;--accent-soft:#3a2418;--neutral:#6d6b63;--good:#5cc08a;--good-soft:#173324;--bad:#f07a6e;--bad-soft:#3a1d1a;--mid:#e0bb4a;--mid-soft:#352c12;color-scheme:dark}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font:15.5px/1.6 var(--sans);-webkit-font-smoothing:antialiased}
main{max-width:1120px;margin:0 auto;padding:48px 16px 96px}
header.top{border-bottom:1px solid var(--line);padding-bottom:28px;margin-bottom:8px}
.eyebrow{font:500 12.5px/1.5 var(--mono);letter-spacing:.02em;color:var(--ink-3)}
h1{font:600 clamp(30px,4.6vw,50px)/1.08 var(--serif);letter-spacing:-.01em;margin:10px 0 14px;text-wrap:balance}
h2{font:600 28px/1.2 var(--serif);margin:64px 0 6px;text-wrap:balance}
h3{font:600 17px/1.35 var(--sans);margin:28px 0 6px}
p{max-width:74ch;margin:0 0 12px}
a{color:var(--accent);text-underline-offset:2px}
.lede{font:400 19px/1.55 var(--serif);color:var(--ink);max-width:70ch}
.verdict{background:var(--accent-soft);border:1px solid var(--line);border-radius:12px;padding:22px 24px;margin:22px 0}
.verdict p{font:400 17px/1.6 var(--serif);max-width:none;margin:0}
.sub{color:var(--ink-2)}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));gap:12px;margin:18px 0}
.tile{background:var(--surface);border:1px solid var(--line);border-radius:10px;padding:14px 16px}
.tile .v{font:600 26px/1.1 var(--mono);font-variant-numeric:tabular-nums}
.tile .k{font-size:13px;color:var(--ink-2);margin-top:6px}
.tile.good .v{color:var(--good)} .tile.bad .v{color:var(--bad)}
.table-wrap{overflow-x:auto;border:1px solid var(--line);border-radius:12px;background:var(--surface);margin:14px 0;padding:6px 10px}
table{border-collapse:collapse;width:100%;font-size:13.5px;font-variant-numeric:tabular-nums}
th,td{padding:8px 10px;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}
thead th{font:500 11.5px/1.3 var(--mono);text-transform:uppercase;letter-spacing:.04em;color:var(--ink-3);background:var(--surface);position:sticky;top:0}
tbody tr:last-child td{border-bottom:0}
tr.trek td{background:var(--accent-soft)}
.nums td{white-space:nowrap}
table.score td{min-width:120px;font-size:12.5px;line-height:1.4}
table.score td:first-child{min-width:110px;font-weight:600;font-size:13px;position:sticky;left:0;background:var(--surface);z-index:1}
table.score tr.trek td:first-child{background:var(--accent-soft)}
table.score thead th:first-child{position:sticky;left:0;z-index:2}
.g{display:inline-block;font:600 11px/1 var(--mono);padding:5px 8px;border-radius:5px;margin-bottom:4px}
.g.ahead{background:var(--good-soft);color:var(--good)} .g.par{background:var(--mid-soft);color:var(--mid)} .g.behind{background:var(--bad-soft);color:var(--bad)} .g.na{background:transparent;color:var(--ink-3);border:1px solid var(--line)}
.chart{margin:18px 0;background:var(--surface);border:1px solid var(--line);border-radius:10px;padding:14px 14px 8px}
.chart-title{font:600 14px/1.3 var(--sans);margin-bottom:8px}
.chart svg{width:100%;height:auto;display:block;font-family:var(--sans)}
.chart .bar{fill:var(--neutral)} .chart .bar.trek{fill:var(--accent)}
.chart .lbl{fill:var(--ink-2);font-size:12.5px} .chart .trek-l{fill:var(--ink);font-weight:700}
.chart .val{fill:var(--ink);font-size:12px;font-family:var(--mono)}
.chart g:hover .bar{opacity:.8}
.chart-note{font-size:12.5px;color:var(--ink-3);margin:8px 2px 4px;max-width:none}
.grid2{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,460px),1fr));gap:16px}
.shots{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,320px),1fr));gap:14px;margin:16px 0}
.shots figure{margin:0;background:var(--surface);border:1px solid var(--line);border-radius:10px;overflow:hidden}
.shots img{width:100%;display:block;aspect-ratio:16/10;object-fit:cover;object-position:top left;background:#000}
.shots figcaption{font-size:12.5px;padding:8px 10px;color:var(--ink-2)}
.pros{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,440px),1fr));gap:16px}
.pros section{background:var(--surface);border:1px solid var(--line);border-radius:10px;padding:16px 18px}
.pros h3{margin-top:0}
.pros li{margin-bottom:8px}
.callout{border:1px solid var(--line);background:var(--surface);border-radius:10px;padding:14px 16px;margin:14px 0}
.callout.warn{background:var(--bad-soft)}
code{font:13px var(--mono);background:var(--surface);border:1px solid var(--line);border-radius:5px;padding:2px 8px}
nav.toc{display:flex;flex-wrap:wrap;gap:6px 14px;font-size:13px;margin-top:16px}
nav.toc a{color:var(--ink-2)}
footer{margin-top:72px;border-top:1px solid var(--line);padding-top:16px;font-size:13px;color:var(--ink-3)}
@media (max-width:640px){h2{font-size:23px}.lede{font-size:17px}}
"""
