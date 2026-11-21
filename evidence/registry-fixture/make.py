"""Builds the fixture ACP Registry the isolated app is pointed at (TREK_ACP_REGISTRY) to check
registry-agent updates end to end: a stand-in ACP agent (a POSIX sh script that answers
`initialize` and `session/new` with a model list, nothing else) at 1.0.0 (3 models) and 1.1.0
(6 models), zipped into archives/, and registry.json listing the version given on the command
line, with each archive's real SHA-256.

    python3 -I make.py 1.0.0   # then start the app, add "Stand-in Agent" from the registry
    python3 -I make.py 1.1.0   # "release" the new version; Settings › Agents › Check now
"""

import hashlib
import json
import sys
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ID = "stand-in-agent"
MODELS = {
    "1.0.0": ["Flash (High)", "Flash (Medium)", "Flash (Low)"],
    "1.1.0": ["Flash (High)", "Flash (Medium)", "Flash (Low)", "Sonnet (Thinking)", "Opus (Thinking)", "OSS 120B (Medium)"],
}

SCRIPT = r"""#!/bin/sh
# A stand-in ACP agent for Trek's isolated app (evidence/registry-fixture/make.py): version VERSION.
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^{[^{]*"id":\([0-9][0-9]*\).*/\1/p')
  [ -z "$id" ] && continue
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"stand-in","version":"VERSION"}}}\n' "$id" ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"stand-in-1","models":{"currentModelId":"m0","availableModels":MODELS}}}\n' "$id" ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
  esac
done
"""


def archive(version: str) -> tuple[str, str]:
    models = json.dumps(
        [{"modelId": f"m{i}", "name": name} for i, name in enumerate(MODELS[version])],
        separators=(",", ":"),
    )
    text = SCRIPT.replace("VERSION", version).replace("MODELS", models.replace("%", "%%"))
    name = f"stand-in-{version}.zip"
    path = HERE / "archives" / name
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as z:
        info = zipfile.ZipInfo("bin/stand-in", date_time=(2026, 11, 20, 0, 0, 0))
        info.external_attr = (0o100755 & 0xFFFF) << 16
        z.writestr(info, text)
    return name, hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    listed = sys.argv[1] if len(sys.argv) > 1 else "1.0.0"
    built = {v: archive(v) for v in MODELS}
    name, sha = built[listed]
    entry = {
        "id": ID,
        "name": "Stand-in Agent",
        "version": listed,
        "description": "A stand-in ACP agent for checking registry updates in the isolated app",
        "authors": ["Trek"],
        "distribution": {
            "binary": {
                target: {
                    "archive": f"https://downloads.example.com/stand-in/{name}",
                    "cmd": "./bin/stand-in",
                    "sha256": sha,
                }
                for target in ("darwin-aarch64", "darwin-x86_64")
            }
        },
    }
    (HERE / "registry.json").write_text(json.dumps({"version": "1.0.0", "agents": [entry]}, indent=1) + "\n")
    (HERE / "icons" / f"{ID}.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="currentColor"/></svg>\n'
    )
    print(f"registry.json lists {ID} {listed} ({name}, sha256 {sha})")


main()
