//! `trek-mcp phone`: the user's real iPhone, through Trek's iPhone tool. Every call goes to the
//! Trek that started the session over its local channel (as `browser` does); Trek holds the phone
//! for one agent at a time, shows it live, and decides what needs the user's approval. See
//! `trek_ipc::phone` for the tools.
//!
//! `trek-mcp phone-device …` is the same phone's apps and files from a shell, without Trek or
//! Xcode (the phone updater installs with it).

use crate::rpc::{self, ToolDef, ToolSet};
use serde_json::Value;
use std::path::Path;

pub struct Phone {
    client: Option<trek_ipc::Client>,
}

impl Phone {
    pub fn from_env() -> Self {
        Self {
            client: trek_ipc::Client::from_env(),
        }
    }
}

impl ToolSet for Phone {
    fn family(&self) -> &'static str {
        "phone"
    }

    fn instructions(&self) -> &'static str {
        trek_ipc::phone::INSTRUCTIONS
    }

    fn tools(&self) -> Vec<ToolDef> {
        trek_ipc::phone::tools()
            .into_iter()
            .map(|(name, description, input_schema)| ToolDef {
                name,
                description,
                input_schema,
            })
            .collect()
    }

    fn call(&mut self, name: &str, args: &Value) -> rpc::ToolResult {
        let client = self.client.as_ref().ok_or(
            "The iPhone tools work only in agents that Trek started (Trek's connection details are missing).",
        )?;
        let result = client.call(name, args)?;
        Ok(crate::browser::content(result))
    }
}

const DEVICE_USAGE: &str = "\
trek-mcp phone-device — the iPhone's apps and files over USB (or Wi-Fi once paired), no Xcode

USAGE:
    trek-mcp phone-device list
    trek-mcp phone-device apps                       [--udid U]
    trek-mcp phone-device install <file.ipa|App.app> [--udid U]
    trek-mcp phone-device ls <path>                  [--app BUNDLE] [--udid U]
    trek-mcp phone-device push <local> <remote>      [--app BUNDLE] [--udid U]
    trek-mcp phone-device pull <remote> <local>      [--app BUNDLE] [--udid U]
    trek-mcp phone-device rm <remote>                [--app BUNDLE] [--udid U]
    trek-mcp phone-device assistivetouch [on|off]    [--udid U]
    trek-mcp phone-device build-helper <out>         (the iPhone tool's helper; needs swiftc)
";

/// Split `--udid U` and `--app B` out of `args`.
fn options(args: &[String]) -> (Vec<String>, Option<String>, Option<String>) {
    let (mut rest, mut udid, mut app) = (vec![], None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--udid" => udid = it.next().cloned(),
            "--app" => app = it.next().cloned(),
            _ => rest.push(a.clone()),
        }
    }
    (rest, udid, app)
}

/// Run `trek-mcp phone-device <args>`; the process's exit code.
pub fn device_cli(args: &[String]) -> i32 {
    use trek_phone::{device, files};
    let (rest, udid, app) = options(args);
    let (udid, app) = (udid.as_deref(), app.as_deref());
    let arg = |i: usize| rest.get(i).map(String::as_str);
    let result: Result<String, String> = match (arg(0), arg(1), arg(2)) {
        (Some("list"), ..) => device::list().map(|phones| {
            if phones.is_empty() {
                return "No iPhone connected.".to_string();
            }
            phones
                .iter()
                .map(|p| {
                    format!(
                        "{}  {}  iOS {}  {}  {}",
                        p.udid,
                        p.name,
                        p.ios,
                        p.model,
                        if p.usb { "USB" } else { "Wi-Fi" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        (Some("apps"), ..) => device::apps(udid).map(|apps| {
            apps.iter()
                .map(|a| {
                    format!(
                        "{}  {}  {} ({}){}",
                        a.bundle,
                        a.name,
                        a.version,
                        a.build,
                        if a.file_sharing {
                            "  [file sharing]"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        (Some("install"), Some(path), _) => {
            device::install(udid, Path::new(path)).map(|()| format!("Installed {path}."))
        }
        (Some("ls"), path, _) => device::list_dir(udid, path.unwrap_or("/"), app).map(|entries| {
            entries
                .iter()
                .map(|e| {
                    if e.dir {
                        format!("{}/", e.name)
                    } else {
                        format!("{}  {}", e.name, files::human_size(e.size))
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        (Some("push"), Some(local), Some(remote)) => {
            device::push(udid, Path::new(local), remote, app)
                .map(|n| format!("Copied {} to {remote}.", files::human_size(n)))
        }
        (Some("pull"), Some(remote), Some(local)) => {
            device::pull(udid, remote, Path::new(local), app)
                .map(|n| format!("Copied {} to {local}.", files::human_size(n)))
        }
        (Some("rm"), Some(remote), _) => {
            device::remove(udid, remote, app).map(|()| format!("Removed {remote}."))
        }
        (Some("assistivetouch"), Some(v @ ("on" | "off")), _) => {
            device::set_assistive_touch(udid, v == "on").map(|()| format!("AssistiveTouch {v}."))
        }
        (Some("assistivetouch"), None, _) => device::assistive_touch(udid)
            .map(|on| format!("AssistiveTouch is {}.", if on { "on" } else { "off" })),
        // For script/bundle.sh: the iPhone tool's helper, compiled into the app bundle.
        (Some("build-helper"), Some(out), _) => {
            trek_phone::helper::build(Path::new(out)).map(|()| format!("Built {out}."))
        }
        _ => {
            eprint!("{DEVICE_USAGE}");
            return 2;
        }
    };
    match result {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
            0
        }
        Err(e) => {
            eprintln!("trek-mcp phone-device: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lists_the_shared_tools_with_their_instructions() {
        let tools = Phone { client: None };
        let names: Vec<&str> = tools.tools().iter().map(|t| t.name).collect();
        assert_eq!(names, trek_ipc::phone::TOOL_NAMES);
        assert!(tools.instructions().contains("phone_screenshot"));
    }

    #[test]
    fn without_trek_calls_say_why() {
        let mut tools = Phone { client: None };
        let err = tools
            .call("phone_tap", &json!({"x": 1, "y": 2}))
            .unwrap_err();
        assert!(err.contains("only in agents that Trek started"), "{err}");
    }

    #[test]
    fn device_options_come_out_of_the_arguments() {
        let args: Vec<String> = [
            "push",
            "a.txt",
            "--app",
            "com.x",
            "/Documents/a.txt",
            "--udid",
            "U1",
        ]
        .map(String::from)
        .to_vec();
        let (rest, udid, app) = options(&args);
        assert_eq!(rest, ["push", "a.txt", "/Documents/a.txt"]);
        assert_eq!(udid.as_deref(), Some("U1"));
        assert_eq!(app.as_deref(), Some("com.x"));
    }
}
