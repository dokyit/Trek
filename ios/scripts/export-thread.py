"""Export a thread's last items, read-only, from a copy of Trek's store, as the phone's wire items
(what `TItem` decodes with convertFromSnakeCase): a fixture for the demo's -TrekFixture.

    sqlite3 -readonly "file:$HOME/Library/Application Support/dev.trek.Trek/trek.sqlite?mode=ro" \\
        ".backup /tmp/trek-copy.sqlite"          # a copy: the live store is never opened
    python3 -I ios/scripts/export-thread.py /tmp/trek-copy.sqlite <thread id> 5000 /tmp/thread.json
    xcrun simctl launch booted dev.trek.TrekMobile -TrekDemo YES -TrekFixture /tmp/thread.json \\
        -TrekOpen t-fixture -TrekScrollProbe YES

The fixture holds the thread's own words: keep it out of the repo."""
import sqlite3, sys, json

db_path, tid, count, out = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
rows = db.execute("select data, created_at from items where thread_id=? order by seq", (tid,)).fetchall()
rows = rows[-count:]


def tool_kind(title):
    if title == "Subagent":
        return "agent"
    if title == "Fetch" or title.startswith("Search the web") or title.startswith("Web"):
        return "web"
    if title.startswith("Run") or title.startswith("Ran"):
        return "command"
    if title.startswith("Edit") or title.startswith("Wr"):
        return "edit"
    if title.startswith("Read"):
        return "read"
    if "Search" in title or title.startswith("List"):
        return "search"
    return "other"


items = []
for n, (data, created) in enumerate(rows):
    d = json.loads(data)
    k = d["kind"]
    it = {"id": f"i{n}", "seq": n + 1}
    if k == "user":
        it.update(kind="user", text=d.get("text", ""), images=len(d.get("images") or []), at=d.get("at"))
    elif k == "assistant":
        it.update(kind="assistant", text=d.get("text", ""), streaming=False)
    elif k == "reasoning":
        it.update(kind="reasoning", text=d.get("text", ""))
    elif k == "tool":
        title = d.get("title", "")
        it.update(kind="tool", call_id=d.get("id", f"c{n}"), tool=tool_kind(title), title=title,
                  detail=d.get("detail", ""), status=d.get("status", "done"),
                  output=(d.get("output") or "")[-4096:])
    elif k == "turn_end":
        it.update(kind="turn_end", took_secs=d.get("took_secs", 0), at=d.get("at"))
    elif k in ("notice", "error"):
        it.update(kind=k, text=d.get("text", ""))
    elif k == "limit":
        it.update(kind="limit", text=d.get("text", ""), resets_at=d.get("resets_at"))
    elif k == "handoff":
        it.update(kind="handoff", **{"from": d.get("from_name") or d.get("from", ""), "to": d.get("to_name") or d.get("to", "")})
    else:
        continue
    if it.get("at") is None:
        it.pop("at", None)
    items.append(it)

with open(out, "w") as f:
    json.dump(items, f)
print(len(items), "items ->", out)
