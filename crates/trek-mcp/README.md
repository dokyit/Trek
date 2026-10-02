# trek-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server that Trek hands to its coding agents
(Claude Code, Codex) so they can see and drive the Mac, or an iOS Simulator.

It speaks JSON-RPC 2.0 over stdio, one message per line (protocol versions `2025-06-18`, `2025-03-26`,
`2024-11-05`; the client's version is echoed back if supported). stdout carries only protocol messages;
logs go to stderr.

```
trek-mcp computer     # macOS computer use
trek-mcp simulator    # iOS Simulator (xcrun simctl + AXe)
```

## Tool families

### `computer`

| Tool | Arguments | What it does |
|---|---|---|
| `screenshot` | `region?: {x,y,width,height}` | PNG of the main display (longest side ≤ 1568 px) plus a text block with the logical screen size and scale. `region` returns a higher-detail zoom. |
| `click` | `x, y, button?: left\|right, count?: 1\|2\|3` | Click at screenshot coordinates. |
| `move_mouse` | `x, y` | Move the pointer. |
| `drag` | `from: {x,y}, to: {x,y}` | Left-button drag. |
| `scroll` | `x, y, dx?, dy?` | Scroll by lines at a point (positive `dy` = down). |
| `type_text` | `text` | Type Unicode text (newline → Return, tab → Tab). |
| `key` | `combo` | `"return"`, `"escape"`, `"cmd+shift+t"`, `"alt+left"`, `"f5"`, … |
| `open_app` | `name` | `open -a <name>`. |
| `list_windows` | – | On-screen windows (owner, title, pid, bounds in points and screenshot pixels). |
| `wait` | `ms ≤ 10000` | Sleep. |

All coordinates are pixels in the latest full-screen screenshot. trek-mcp converts them to screen points
with that screenshot's scale, so the agent never has to think about Retina or downscaling.

Permissions belong to the process that launched trek-mcp (Trek, or the terminal running the agent):

- **Screen & System Audio Recording** for `screenshot` (and window titles in `list_windows`).
- **Accessibility** for `click`, `move_mouse`, `drag`, `scroll`, `type_text`, `key`.

When either is missing the tool returns an MCP error (`isError: true`) saying which permission to grant
in System Settings ▸ Privacy & Security.

### `simulator`

| Tool | Arguments | Backend |
|---|---|---|
| `sim_list` | – | `simctl list devices available -j` |
| `sim_boot` / `sim_shutdown` | `udid` | `simctl boot` (+ `bootstatus`) / `simctl shutdown` |
| `sim_screenshot` | `udid?` | `simctl io <udid> screenshot`, resized to device points |
| `sim_open_url` | `url, udid?` | `simctl openurl` |
| `sim_install` | `app_path, udid?` | `simctl install` |
| `sim_launch` | `bundle_id, udid?` | `simctl launch --terminate-running-process` |
| `sim_tap` | `x, y, udid?` | `axe tap` |
| `sim_swipe` | `from, to, duration?, udid?` | `axe swipe` |
| `sim_type` | `text, udid?` | `axe type --stdin` |
| `sim_button` | `name: home\|lock\|siri\|side-button, udid?` | `axe button` |

`udid` defaults to the booted simulator. Simulator screenshots are sized in device points (the
device's screen scale comes from its CoreSimulator device-type profile), so screenshot coordinates can
be passed straight to `sim_tap` / `sim_swipe`.

Touch, typing and buttons need [AXe](https://github.com/cameroncooke/AXe). trek-mcp looks for `axe` in
`$TREK_AXE_PATH`, then `PATH`, `/opt/homebrew/bin`, `/usr/local/bin` and `~/.local/bin`. If it is missing
those tools return: *"Touch input needs AXe. Turn on 'Simulator touch input' in Trek ▸ Settings ▸ Tools to
install it."*

## How Trek passes it to agents

Trek ships the `trek-mcp` binary next to the app executable and adds one server per enabled tool family.

### Claude Code

Write an MCP config file and pass it with `--mcp-config` when spawning `claude`:

```json
{
  "mcpServers": {
    "trek-computer": { "command": "/path/to/trek-mcp", "args": ["computer"] },
    "trek-simulator": { "command": "/path/to/trek-mcp", "args": ["simulator"] }
  }
}
```

```
claude --mcp-config /path/to/trek-mcp.json …
```

The tools then appear to the model as `mcp__trek-computer__screenshot`, `mcp__trek-simulator__sim_tap`, and
so on. Add them to `--allowedTools` (e.g. `mcp__trek-computer`) if the session should not prompt for each call.

### Codex

Codex reads MCP servers from the `mcp_servers` table of its config. Trek sets it per thread through the
thread config (the same shape as `~/.codex/config.toml`):

```toml
[mcp_servers.trek-computer]
command = "/path/to/trek-mcp"
args = ["computer"]

[mcp_servers.trek-simulator]
command = "/path/to/trek-mcp"
args = ["simulator"]
```

Over the app-server protocol this is the `config` object on `thread/start`:

```json
{ "mcp_servers": { "trek-computer": { "command": "/path/to/trek-mcp", "args": ["computer"] } } }
```

or, on the command line, `codex -c 'mcp_servers.trek-computer.command="/path/to/trek-mcp"' -c 'mcp_servers.trek-computer.args=["computer"]'`.

## Trying it by hand

```sh
cargo build -p trek-mcp
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_windows","arguments":{}}}' \
  | target/debug/trek-mcp computer
```
