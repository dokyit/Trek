//! Reading MCP servers the way they're handed around: a command line, a URL, a JSON snippet
//! from a server's README (Claude Desktop, Cursor, VS Code, Windsurf, Zed, Gemini), or a
//! `claude mcp add` / `codex mcp add` line. Everything comes out as `McpServerConfig`s with
//! their env and header values inline; the caller moves those to the Keychain.

use crate::settings::{McpEnvVar, McpHeader, McpServerConfig, is_mcp_url};
use serde_json::{Map, Value};

/// Read what the user typed or pasted into the add row as one or more servers. `name` is the
/// name field: it names a single server, and may be left empty when the text names its
/// servers (a JSON map, a `claude mcp add` line) or a name can be told from the command or URL.
pub fn read_servers(name: &str, text: &str) -> Result<Vec<McpServerConfig>, String> {
    let name = name.trim();
    // A line copied from a terminal prompt.
    let text = text.trim().trim_start_matches("$ ").trim();
    if text.is_empty() {
        return Err("Type a command or a URL, or paste a server's JSON config.".into());
    }
    let named = if text.starts_with('{') || text.starts_with('"') {
        from_json_text(text)?
    } else {
        let words = split_words(text)?;
        match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            ["claude", "mcp", "add-json", ..] => claude_add_json(&words[3..])?,
            ["claude", "mcp", "add", ..] => vec![claude_add(&words[3..])?],
            ["codex", "mcp", "add", ..] => vec![codex_add(&words[3..])?],
            _ => vec![(None, McpServerConfig::parse("-", text).ok_or("Type a command or a URL, or paste a server's JSON config.")?)],
        }
    };
    let single = named.len() == 1;
    named
        .into_iter()
        .map(|(given, mut server)| {
            // The name field wins for a single server; otherwise the config's own name, or one
            // told from what it runs.
            server.name = match (single && !name.is_empty(), given) {
                (true, _) => name.to_string(),
                (false, Some(n)) if !n.trim().is_empty() => n.trim().to_string(),
                _ => suggest_name(&server).ok_or("Give the server a name.")?,
            };
            Ok(server)
        })
        .collect()
}

/// Split a command line into words as a shell would: '…' and "…" quote, a backslash escapes the
/// next character. A backslash that starts a word before a space is a line continuation (a
/// multi-line command pasted into one line).
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = vec![];
    let mut word: Option<String> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                words.extend(word.take());
            }
            '\\' if word.is_none() && chars.peek().is_none_or(|n| n.is_whitespace()) => {}
            '\\' => {
                if let Some(n) = chars.next() {
                    word.get_or_insert_default().push(n);
                }
            }
            '\'' => {
                let w = word.get_or_insert_default();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => w.push(c),
                        None => return Err("A quote isn't closed.".into()),
                    }
                }
            }
            '"' => {
                let w = word.get_or_insert_default();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if chars.peek().is_some_and(|n| matches!(n, '"' | '\\' | '$' | '`')) => w.push(chars.next().unwrap_or_default()),
                        Some(c) => w.push(c),
                        None => return Err("A quote isn't closed.".into()),
                    }
                }
            }
            c => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    Ok(words)
}

/// `KEY=value` (a shell variable assignment), as its two halves.
pub fn assignment(word: &str) -> Option<(&str, &str)> {
    let (k, v) = word.split_once('=')?;
    let mut chars = k.chars();
    (chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some((k, v))
}

/// A word written so `split_words` reads it back as it is.
pub fn quote(word: &str) -> String {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,~^".contains(c)) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// A name for a server from what it runs: the package (`@modelcontextprotocol/server-github`
/// → `github`, `mcp-server-fetch` → `fetch`) or the host (`mcp.notion.com` → `notion`).
pub fn suggest_name(server: &McpServerConfig) -> Option<String> {
    let raw = match &server.url {
        Some(url) => {
            let host = url.split("://").nth(1)?.split(['/', ':', '?']).next()?;
            // An address on this Mac (`localhost`, `127.0.0.1`) says nothing about the server.
            if host == "localhost" || host.parse::<std::net::IpAddr>().is_ok() {
                return None;
            }
            let parts: Vec<&str> = host.split('.').filter(|p| !matches!(*p, "mcp" | "www" | "api")).collect();
            match parts[..] {
                [] => return None,
                [only] => only.to_string(),
                // The name before the top-level domain (`notion` of notion.com).
                [.., name, _] => name.to_string(),
            }
        }
        None => {
            // The word that names an MCP server (`npx -y @x/server-y`, `uvx mcp-server-fetch`,
            // `docker run … ghcr.io/github/github-mcp-server`), or else the last that isn't a flag.
            let words: Vec<&String> = server.args.iter().filter(|a| !a.starts_with('-') && !a.contains('=')).collect();
            let named = |w: &&&String| ["mcp", "server"].iter().any(|k| w.to_ascii_lowercase().contains(k));
            let word = words.iter().rev().find(named).or(words.last()).copied().unwrap_or(&server.command);
            let word = word.rsplit('/').next()?;
            let word = word.split('@').find(|p| !p.is_empty())?;
            // A script's name without its extension.
            let word = match word.rsplit_once('.') {
                Some((stem, ext)) if ["js", "mjs", "cjs", "ts", "py", "pl", "rb", "sh", "jar"].contains(&ext) => stem,
                _ => word,
            };
            word.to_string()
        }
    };
    let mut name = raw.to_ascii_lowercase();
    for affix in ["mcp-server-", "server-", "mcp-"] {
        name = name.strip_prefix(affix).map(str::to_string).unwrap_or(name);
    }
    for affix in ["-mcp-server", "-server", "-mcp"] {
        name = name.strip_suffix(affix).map(str::to_string).unwrap_or(name);
    }
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    (!name.is_empty() && name != "-").then_some(name)
}

/// The env vars and headers of `server` whose value is still the README's stand-in
/// (`<YOUR_TOKEN>`, `your-api-key`), not a real one.
pub fn placeholders(server: &McpServerConfig) -> Vec<String> {
    let looks = |v: &str| {
        let l = v.to_ascii_lowercase();
        (v.starts_with('<') && v.ends_with('>')) || l.contains("your_") || l.contains("your-") || l.contains("<your") || l == "..." || l == "xxx" || l.contains("${input:")
    };
    let env = server.env.iter().filter(|e| looks(&e.value)).map(|e| e.name.clone());
    env.chain(server.headers.iter().filter(|h| looks(&h.value)).map(|h| h.name.clone())).collect()
}

type Named = Vec<(Option<String>, McpServerConfig)>;

fn from_json_text(text: &str) -> Result<Named, String> {
    let value = serde_json::from_str::<Value>(text)
        .or_else(|e| {
            // A piece cut from inside `mcpServers`: `"github": {…}`, maybe with its comma.
            let piece = text.trim_end().trim_end_matches(',');
            serde_json::from_str::<Value>(&format!("{{{piece}}}")).map_err(|_| e)
        })
        .map_err(|e| format!("That JSON doesn't read: {e}."))?;
    from_json(&value)
}

/// The servers in a JSON config, whichever app's shape it has.
fn from_json(value: &Value) -> Result<Named, String> {
    let Some(obj) = value.as_object() else { return Err("That JSON isn't an MCP server config.".into()) };
    // The map of servers under the key an app keeps it in.
    let map = ["mcpServers", "servers", "context_servers", "mcp_servers"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_object))
        .or_else(|| obj.get("mcp").and_then(|m| m.get("servers")).and_then(Value::as_object));
    if let Some(map) = map {
        return servers_of(map);
    }
    if is_server(obj) {
        return Ok(vec![(obj.get("name").and_then(Value::as_str).map(str::to_string), server_from_json("", obj)?)]);
    }
    if !obj.is_empty() && obj.values().all(|v| v.as_object().is_some_and(is_server)) {
        return servers_of(obj);
    }
    Err("That JSON has no MCP servers in it: no command or url.".into())
}

fn servers_of(map: &Map<String, Value>) -> Result<Named, String> {
    let named: Named = map
        .iter()
        .map(|(name, v)| {
            let obj = v.as_object().ok_or_else(|| format!("{name} isn't a server config."))?;
            Ok((Some(name.clone()), server_from_json(name, obj)?))
        })
        .collect::<Result<_, String>>()?;
    if named.is_empty() {
        return Err("That config lists no servers.".into());
    }
    Ok(named)
}

fn is_server(obj: &Map<String, Value>) -> bool {
    ["command", "url", "serverUrl", "httpUrl"].iter().any(|k| obj.contains_key(*k))
}

/// A string, or a number or true/false written as one (`"PORT": 8080`).
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(_) | Value::Bool(_) => Some(v.to_string()),
        _ => None,
    }
}

fn pairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), text(v)?))).collect()).unwrap_or_default()
}

fn server_from_json(name: &str, obj: &Map<String, Value>) -> Result<McpServerConfig, String> {
    let label = if name.is_empty() { "The server" } else { name };
    let enabled = obj.get("disabled") != Some(&Value::Bool(true)) && obj.get("enabled") != Some(&Value::Bool(false));
    let url = ["url", "serverUrl", "httpUrl"].iter().find_map(|k| obj.get(*k).and_then(Value::as_str));
    if let Some(url) = url {
        if !is_mcp_url(url) {
            return Err(format!("{label}'s url isn't an http(s) address."));
        }
        let headers = pairs(obj.get("headers")).into_iter().map(|(name, value)| McpHeader { name, value, secret: false }).collect();
        return Ok(McpServerConfig { enabled, ..McpServerConfig::http(name, url, headers) });
    }
    // Zed's older shape: `"command": {"path", "args", "env"}`.
    let (obj, command) = match obj.get("command") {
        Some(Value::Object(inner)) => (inner, inner.get("path").and_then(Value::as_str)),
        Some(Value::String(c)) => (obj, Some(c.as_str())),
        _ => (obj, None),
    };
    let command = command.map(str::trim).filter(|c| !c.is_empty()).ok_or_else(|| format!("{label} has no command or url."))?;
    let mut args: Vec<String> = obj.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(text).collect()).unwrap_or_default();
    let mut command = command.to_string();
    // `"command": "uvx mcp-server-fetch"`, args and all in one string.
    if args.is_empty() && command.contains(' ') && !std::path::Path::new(&command).exists() {
        let mut words = split_words(&command)?.into_iter();
        command = words.next().unwrap_or_default();
        args = words.collect();
    }
    let env = pairs(obj.get("env")).into_iter().map(|(name, value)| McpEnvVar { name, value, secret: false }).collect();
    Ok(McpServerConfig { env, enabled, ..McpServerConfig::stdio(name, command, args) })
}

/// `claude mcp add [options] <name> <commandOrUrl> [args…]`, or with `-- <command> [args…]`.
/// `--env` and `--header` each take one or more values.
fn claude_add(words: &[String]) -> Result<(Option<String>, McpServerConfig), String> {
    let mut transport = None;
    let (mut env, mut headers, mut rest) = (vec![], vec![], None);
    let mut positional: Vec<String> = vec![];
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        i += 1;
        // Past the name and the command, every word is one of the command's arguments (a
        // URL has none: its `--header` may come after it).
        if positional.len() >= 2 && !is_mcp_url(&positional[1]) {
            positional.push(w.to_string());
            continue;
        }
        let (flag, inline) = match w.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (w, None),
        };
        match flag {
            "--" => {
                rest = Some(words[i..].to_vec());
                break;
            }
            "-t" | "--transport" => transport = inline.or_else(|| words.get(i).cloned().inspect(|_| i += 1)),
            "-e" | "--env" => {
                env.extend(inline.and_then(|v| assignment(&v).map(|(k, v)| (k.to_string(), v.to_string()))));
                while let Some((k, v)) = words.get(i).and_then(|w| assignment(w)) {
                    env.push((k.to_string(), v.to_string()));
                    i += 1;
                }
            }
            "-H" | "--header" => {
                headers.extend(inline.and_then(|v| McpHeader::parse(&v)));
                while let Some(h) = words.get(i).and_then(|w| McpHeader::parse(w)) {
                    headers.push(h);
                    i += 1;
                }
            }
            "-s" | "--scope" | "--client-id" | "--callback-port" => {
                if inline.is_none() {
                    i += 1;
                }
            }
            f if f.starts_with('-') && positional.is_empty() => {}
            _ => positional.push(w.to_string()),
        }
    }
    let mut positional = positional.into_iter();
    let name = positional.next().ok_or("That claude mcp add line has no server name.")?;
    let mut run: Vec<String> = rest.unwrap_or_default();
    if run.is_empty() {
        run = positional.collect();
    }
    let mut run = run.into_iter();
    let target = run.next().ok_or_else(|| format!("That claude mcp add line gives {name} no command or URL."))?;
    let remote = matches!(transport.as_deref(), Some("http" | "sse")) || is_mcp_url(&target);
    let server = if remote {
        let headers = headers.into_iter().map(|(name, value)| McpHeader { name, value, secret: false }).collect();
        McpServerConfig::http(&name, target, headers)
    } else {
        let env = env.into_iter().map(|(name, value)| McpEnvVar { name, value, secret: false }).collect();
        McpServerConfig { env, ..McpServerConfig::stdio(&name, target, run.collect()) }
    };
    Ok((Some(name), server))
}

/// `claude mcp add-json [-s scope] <name> '<json>'`.
fn claude_add_json(words: &[String]) -> Result<Named, String> {
    let mut plain = vec![];
    let mut i = 0;
    while i < words.len() {
        match words[i].as_str() {
            "-s" | "--scope" => i += 1,
            w if w.starts_with('-') => {}
            w => plain.push(w),
        }
        i += 1;
    }
    let [name, json] = plain[..] else { return Err("That claude mcp add-json line needs a name and the JSON.".into()) };
    let mut servers = from_json_text(json)?;
    if let [(given, _)] = &mut servers[..] {
        *given = Some(name.to_string());
    }
    Ok(servers)
}

/// `codex mcp add <name> [--env K=V]… -- <command> [args…]`, or `codex mcp add <name> --url <url>`.
fn codex_add(words: &[String]) -> Result<(Option<String>, McpServerConfig), String> {
    let (mut name, mut url, mut env, mut run) = (None, None, vec![], vec![]);
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        i += 1;
        let (flag, inline) = match w.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (w, None),
        };
        let mut value = || inline.clone().or_else(|| words.get(i).cloned().inspect(|_| i += 1));
        match flag {
            "--" => {
                run = words[i..].to_vec();
                break;
            }
            "--url" => url = value(),
            "--env" => {
                if let Some(v) = value() {
                    env.extend(assignment(&v).map(|(k, v)| McpEnvVar { name: k.into(), value: v.into(), secret: false }));
                }
            }
            "--bearer-token-env-var" => {
                let _ = value();
            }
            f if f.starts_with('-') => {}
            _ if name.is_none() => name = Some(w.to_string()),
            _ => run.push(w.to_string()),
        }
    }
    let name = name.ok_or("That codex mcp add line has no server name.")?;
    if let Some(url) = url {
        return Ok((Some(name.clone()), McpServerConfig::http(name, url, vec![])));
    }
    let mut run = run.into_iter();
    let command = run.next().ok_or_else(|| format!("That codex mcp add line gives {name} no command."))?;
    Ok((Some(name.clone()), McpServerConfig { env, ..McpServerConfig::stdio(name, command, run.collect()) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(s: &McpServerConfig) -> Vec<(&str, &str)> {
        s.env.iter().map(|e| (e.name.as_str(), e.value.as_str())).collect()
    }

    fn one(name: &str, text: &str) -> McpServerConfig {
        let mut all = read_servers(name, text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(all.len(), 1, "{text}");
        all.remove(0)
    }

    #[test]
    fn command_lines_split_as_a_shell_would() {
        assert_eq!(split_words(r#"node "/Users/me/My Servers/x.js" --flag='a b' c\ d"#).unwrap(), ["node", "/Users/me/My Servers/x.js", "--flag=a b", "c d"]);
        assert_eq!(split_words(r#"echo "say \"hi\"" '' x"#).unwrap(), ["echo", "say \"hi\"", "", "x"]);
        // A multi-line command pasted into one line keeps its continuations as nothing.
        assert_eq!(split_words("npx -y \\  @scope/server \\").unwrap(), ["npx", "-y", "@scope/server"]);
        assert!(split_words("echo 'open").is_err());
        for w in ["plain", "with space", "it's", "", "a\"b"] {
            assert_eq!(split_words(&quote(w)).unwrap(), [w]);
        }
    }

    #[test]
    fn a_command_line_takes_its_environment_from_the_front() {
        let s = one("gh", "GITHUB_PERSONAL_ACCESS_TOKEN=ghp_x LOG='a b' npx -y @modelcontextprotocol/server-github");
        assert_eq!((s.command.as_str(), s.args.clone()), ("npx", vec!["-y".to_string(), "@modelcontextprotocol/server-github".into()]));
        assert_eq!(env(&s), [("GITHUB_PERSONAL_ACCESS_TOKEN", "ghp_x"), ("LOG", "a b")]);
        assert_eq!(env(&one("x", "env A=1 run-it --port=3")), [("A", "1")]);
        // An argument that looks like one, after the command, stays an argument.
        assert_eq!(one("x", "run A=1").args, ["A=1"]);
        // No name typed: one is told from the package or the host.
        assert_eq!(one("", "npx -y @modelcontextprotocol/server-github").name, "github");
        assert_eq!(one("", "uvx mcp-server-fetch").name, "fetch");
        assert_eq!(one("", "node /srv/weather-mcp.js --port 3000").name, "weather");
        assert_eq!(one("", "docker run -i --rm -e GITHUB_TOKEN ghcr.io/github/github-mcp-server stdio").name, "github");
        assert_eq!(one("", "https://mcp.notion.com/mcp").name, "notion");
        assert!(read_servers("", "http://127.0.0.1:3845/mcp").is_err(), "a local address names nothing");
        assert_eq!(one("mine", "https://mcp.linear.app/mcp").name, "mine", "a typed name wins");
        assert!(read_servers("", "  ").is_err());
        assert!(read_servers("x", "run 'open").is_err());
    }

    #[test]
    fn claude_desktop_and_cursor_configs_read_every_server() {
        let text = r#"{
          "mcpServers": {
            "github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "<YOUR_TOKEN>"}},
            "linear": {"url": "https://mcp.linear.app/mcp", "headers": {"Authorization": "Bearer lin_x"}},
            "off": {"command": "uvx", "args": ["mcp-server-time"], "disabled": true, "env": {"PORT": 8080}}
          }
        }"#;
        // A single-line field drops the newlines of a paste; it reads the same.
        for text in [text.to_string(), text.replace('\n', "")] {
            let all = read_servers("ignored", &text).unwrap();
            assert_eq!(all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["github", "linear", "off"]);
            assert_eq!(env(&all[0]), [("GITHUB_PERSONAL_ACCESS_TOKEN", "<YOUR_TOKEN>")]);
            assert_eq!(placeholders(&all[0]), ["GITHUB_PERSONAL_ACCESS_TOKEN"]);
            assert_eq!(all[1].url.as_deref(), Some("https://mcp.linear.app/mcp"));
            assert_eq!(all[1].headers, [McpHeader { name: "Authorization".into(), value: "Bearer lin_x".into(), secret: false }]);
            assert!(placeholders(&all[1]).is_empty());
            assert!(!all[2].enabled, "a disabled one comes in turned off");
            assert_eq!(env(&all[2]), [("PORT", "8080")]);
        }
    }

    #[test]
    fn vs_code_windsurf_zed_and_gemini_shapes_read_too() {
        // VS Code: `servers`, typed, with its `inputs` alongside.
        let vs = read_servers("", r#"{"inputs": [], "servers": {"fetch": {"type": "stdio", "command": "uvx", "args": ["mcp-server-fetch"]}, "gh": {"type": "http", "url": "https://api.githubcopilot.com/mcp/"}}}"#).unwrap();
        assert_eq!((vs[0].name.as_str(), vs[0].command.as_str(), vs[1].name.as_str(), vs[1].is_http()), ("fetch", "uvx", "gh", true));
        // VS Code's settings.json nests it under `mcp`.
        assert_eq!(read_servers("", r#"{"mcp": {"servers": {"a": {"command": "a-server"}}}}"#).unwrap()[0].command, "a-server");
        // Windsurf's `serverUrl`, Gemini's `httpUrl`.
        assert_eq!(one("w", r#"{"serverUrl": "https://example.com/mcp"}"#).url.as_deref(), Some("https://example.com/mcp"));
        assert_eq!(one("g", r#"{"httpUrl": "https://example.com/mcp"}"#).url.as_deref(), Some("https://example.com/mcp"));
        // Zed: `context_servers`, older ones with the command as an object.
        let zed = one("", r#"{"context_servers": {"pg": {"command": {"path": "pg-mcp", "args": ["--db", "x"], "env": {"PGPASS": "p"}}}}}"#);
        assert_eq!((zed.name.as_str(), zed.command.as_str(), zed.args.len(), env(&zed)), ("pg", "pg-mcp", 2, vec![("PGPASS", "p")]));
        // A bare map of servers, and one server on its own (named by the field, or told).
        assert_eq!(read_servers("", r#"{"a": {"command": "x"}, "b": {"url": "https://b.dev/mcp"}}"#).unwrap().len(), 2);
        assert_eq!(one("srv", r#"{"command": "node", "args": ["build/index.js"]}"#).name, "srv");
        assert_eq!(one("", r#"{"type": "http", "url": "https://mcp.sentry.dev/mcp"}"#).name, "sentry");
        // A piece cut from inside `mcpServers`, trailing comma and all.
        let piece = one("", r#""github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"]},"#);
        assert_eq!((piece.name.as_str(), piece.args.len()), ("github", 2));
        // The whole command in one string.
        assert_eq!(one("f", r#"{"command": "uvx mcp-server-fetch --x"}"#).args, ["mcp-server-fetch", "--x"]);
        // What isn't a server says so.
        assert!(read_servers("x", r#"{"theme": "dark"}"#).is_err());
        assert!(read_servers("x", r#"{"mcpServers": {"x": {"args": []}}}"#).unwrap_err().contains("no command"));
        assert!(read_servers("x", "{not json").is_err());
    }

    #[test]
    fn claude_and_codex_add_lines_read_as_their_clis_do() {
        let s = one("", "claude mcp add --transport stdio --env AIRTABLE_API_KEY=key123 -e B=2 airtable -- npx -y airtable-mcp-server");
        assert_eq!((s.name.as_str(), s.command.as_str(), s.args.clone()), ("airtable", "npx", vec!["-y".to_string(), "airtable-mcp-server".into()]));
        assert_eq!(env(&s), [("AIRTABLE_API_KEY", "key123"), ("B", "2")]);
        // `-e` takes several, the scope is passed over, no `--`.
        let s = one("", "claude mcp add -s user my-server -e A=1 B=2 /path/to/server arg1");
        assert_eq!((s.name.as_str(), s.command.as_str(), s.args.clone(), env(&s).len()), ("my-server", "/path/to/server", vec!["arg1".to_string()], 2));
        let s = one("", r#"$ claude mcp add --transport http notion https://mcp.notion.com/mcp --header "Authorization: Bearer abc""#);
        assert_eq!((s.name.as_str(), s.url.as_deref(), s.headers.len()), ("notion", Some("https://mcp.notion.com/mcp"), 1));
        assert_eq!(s.headers[0].value, "Bearer abc");
        let s = one("", r#"claude mcp add-json weather '{"type":"stdio","command":"/path/to/weather-cli","args":["--api-key","abc123"],"env":{"CACHE_DIR":"/tmp"}}'"#);
        assert_eq!((s.name.as_str(), s.args.len(), env(&s)), ("weather", 2, vec![("CACHE_DIR", "/tmp")]));

        let s = one("", "codex mcp add context7 --env K=v -- npx -y @upstash/context7-mcp");
        assert_eq!((s.name.as_str(), s.command.as_str(), s.args.len(), env(&s)), ("context7", "npx", 2, vec![("K", "v")]));
        let s = one("", "codex mcp add figma --url https://mcp.figma.com/mcp --bearer-token-env-var FIGMA_TOKEN");
        assert_eq!((s.name.as_str(), s.url.as_deref()), ("figma", Some("https://mcp.figma.com/mcp")));
        assert!(read_servers("", "claude mcp add").is_err());
    }

    #[test]
    fn an_edited_server_shows_its_saved_values_as_kept() {
        let mut s = one("gh", "TOKEN=abc 'my server' --dir '/a b'");
        assert_eq!(s.command_line(), "TOKEN=abc 'my server' --dir '/a b'");
        s.env[0].secret = true;
        s.env[0].value.clear();
        assert_eq!(s.command_line(), format!("TOKEN={} 'my server' --dir '/a b'", crate::settings::KEPT));
        // Read back, the kept value is still `KEPT` for `stash_secrets` to fill in.
        assert_eq!(env(&one("gh", &s.command_line())), [("TOKEN", crate::settings::KEPT)]);
    }
}
