//! What Trek can see of the Mac's tool setup: privacy permissions for computer use, the AXe
//! touch driver for the simulator, and the MCP servers, skills and plugins agents already have.

use std::path::PathBuf;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

/// Accessibility permission: needed to click and type for the agent.
pub fn accessibility_allowed() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Screen Recording permission: needed for screenshots of other apps.
pub fn screen_recording_allowed() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub const ACCESSIBILITY_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const SCREEN_RECORDING_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// AXe drives simulator touches (taps, swipes, typing).
pub fn axe_path() -> Option<PathBuf> {
    std::env::var_os("TREK_AXE_PATH").map(PathBuf::from).filter(|p| p.exists()).or_else(|| trek_core::detect::which("axe"))
}

pub const AXE_INSTALL: &str = "brew install cameroncooke/axe/axe";

/// MCP servers each agent already loads from its own config: `(agent, server names)`.
pub fn agent_mcp_servers() -> Vec<(&'static str, Vec<String>)> {
    let home = trek_core::paths::agents_home();
    let mut out = vec![];
    // Claude Code: user-level `mcpServers` plus per-project ones in ~/.claude.json.
    if let Ok(text) = std::fs::read_to_string(home.join(".claude.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            let mut names: Vec<String> = v["mcpServers"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
            if let Some(projects) = v["projects"].as_object() {
                for p in projects.values() {
                    if let Some(m) = p["mcpServers"].as_object() {
                        names.extend(m.keys().cloned());
                    }
                }
            }
            names.sort();
            names.dedup();
            out.push(("Claude Code", names));
        }
    }
    // Codex: `[mcp_servers.<name>]` tables in ~/.codex/config.toml.
    if let Ok(text) = std::fs::read_to_string(home.join(".codex/config.toml")) {
        let mut names: Vec<String> = text
            .lines()
            .filter_map(|l| l.trim().strip_prefix("[mcp_servers.")?.strip_suffix(']').map(str::to_string))
            .filter(|n| !n.contains('.'))
            .map(|n| n.trim_matches('"').to_string())
            .collect();
        names.dedup();
        out.push(("Codex", names));
    }
    out
}

/// Claude Code plugins the user has installed (`name@marketplace`).
pub fn claude_plugins() -> Vec<String> {
    let path = trek_core::paths::agents_home().join(".claude/plugins/installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return vec![] };
    let mut names: Vec<String> = v["plugins"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    names.sort();
    names
}
