//! `ccnotify setup` / `ccnotify uninstall`: install the hook script,
//! wire the hook entries into global ~/.claude/settings.json, and add a
//! `claude` shell alias pointing at the wrapper.

use std::fs;
use std::path::PathBuf;

const HOOK_SCRIPT: &str = include_str!("../../hooks/notify-forward.sh");
const HOOK_SCRIPT_NAME: &str = "ccnotify-forward.sh";
const RC_BEGIN: &str = "# >>> ccnotify >>>";
const RC_END: &str = "# <<< ccnotify <<<";

/// Hook events we subscribe to, with per-event timeout (seconds).
/// PreToolUse gets a long timeout because the hook call stays blocked
/// while the person decides from the overlay.
const HOOK_EVENTS: &[(&str, Option<&str>, u64)] = &[
    ("PreToolUse", Some("*"), 600),
    ("Notification", None, 30),
    ("Stop", None, 30),
    ("UserPromptSubmit", None, 10),
];

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

pub fn setup() -> Result<(), String> {
    let home = home()?;
    let hooks_dir = home.join(".claude/hooks");
    let hook_path = hooks_dir.join(HOOK_SCRIPT_NAME);
    let settings_path = home.join(".claude/settings.json");

    // 1. Hook script.
    fs::create_dir_all(&hooks_dir).map_err(|e| e.to_string())?;
    fs::write(&hook_path, HOOK_SCRIPT).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }

    // 2. Global settings.json hook entries.
    let mut settings: serde_json::Value = match fs::read_to_string(&settings_path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| format!("{} is not valid JSON: {e}", settings_path.display()))?,
        Err(_) => serde_json::json!({}),
    };
    let hook_cmd = hook_path.to_string_lossy().into_owned();
    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json top level is not an object")?
        .entry("hooks")
        .or_insert(serde_json::json!({}));
    let hooks = hooks.as_object_mut().ok_or("settings.json `hooks` is not an object")?;

    for (event, matcher, timeout) in HOOK_EVENTS {
        let entries = hooks
            .entry(event.to_string())
            .or_insert(serde_json::json!([]));
        let entries = entries
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{event} is not an array"))?;
        if entries.iter().any(is_ours) {
            continue; // already installed
        }
        let mut group = serde_json::json!({
            "hooks": [{ "type": "command", "command": hook_cmd, "timeout": timeout }]
        });
        if let Some(m) = matcher {
            group["matcher"] = serde_json::json!(m);
        }
        entries.push(group);
    }
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).unwrap() + "\n",
    )
    .map_err(|e| e.to_string())?;

    // 3. Shell alias.
    let rc = rc_file(&home)?;
    let alias_line = if rc.ends_with("config.fish") {
        "alias claude \"ccnotify claude\""
    } else {
        "alias claude=\"ccnotify claude\""
    };
    let existing = fs::read_to_string(&rc).unwrap_or_default();
    if !existing.contains(RC_BEGIN) {
        if let Some(parent) = rc.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let block = format!("\n{RC_BEGIN}\n{alias_line}\n{RC_END}\n");
        fs::write(&rc, existing + &block).map_err(|e| e.to_string())?;
    }

    println!("ccnotify setup complete. Changes made:");
    println!("  1. wrote hook script     {}", hook_path.display());
    println!(
        "  2. added hook entries    {} (PreToolUse, Notification, Stop, UserPromptSubmit)",
        settings_path.display()
    );
    println!("  3. added shell alias     {} (alias claude=\"ccnotify claude\")", rc.display());
    println!();
    println!("Open a new terminal (or `source {}`), then `claude` runs through ccnotify.", rc.display());
    println!("Undo everything with: ccnotify uninstall");
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let home = home()?;
    let hook_path = home.join(".claude/hooks").join(HOOK_SCRIPT_NAME);
    let settings_path = home.join(".claude/settings.json");

    // 1. Hook entries out of settings.json.
    if let Ok(raw) = fs::read_to_string(&settings_path) {
        if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                for (event, _, _) in HOOK_EVENTS {
                    if let Some(entries) = hooks.get_mut(*event).and_then(|e| e.as_array_mut()) {
                        entries.retain(|g| !is_ours(g));
                    }
                }
                hooks.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
            }
            let _ = fs::write(
                &settings_path,
                serde_json::to_string_pretty(&settings).unwrap() + "\n",
            );
        }
    }

    // 2. Hook script.
    let _ = fs::remove_file(&hook_path);

    // 3. Shell alias block.
    if let Ok(rc) = rc_file(&home) {
        if let Ok(existing) = fs::read_to_string(&rc) {
            if let (Some(start), Some(end)) = (existing.find(RC_BEGIN), existing.find(RC_END)) {
                if start < end {
                    let mut cleaned = String::new();
                    cleaned.push_str(existing[..start].trim_end_matches('\n'));
                    if !cleaned.is_empty() {
                        cleaned.push('\n');
                    }
                    let rest = &existing[end + RC_END.len()..];
                    cleaned.push_str(rest.trim_start_matches('\n'));
                    let _ = fs::write(&rc, cleaned);
                }
            }
        }
    }

    println!("ccnotify uninstall complete: hook entries, hook script, and shell alias removed.");
    println!("Existing terminals may still have the old alias loaded until reopened.");
    Ok(())
}

/// A settings.json hook group counts as ours when every command in it
/// points at our forwarder script.
fn is_ours(group: &serde_json::Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hs| {
            !hs.is_empty()
                && hs.iter().all(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.contains(HOOK_SCRIPT_NAME))
                        .unwrap_or(false)
                })
        })
        .unwrap_or(false)
}

fn rc_file(home: &std::path::Path) -> Result<PathBuf, String> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let shell = shell.rsplit('/').next().unwrap_or("");
    match shell {
        "zsh" => Ok(home.join(".zshrc")),
        "bash" => Ok(home.join(".bashrc")),
        "fish" => Ok(home.join(".config/fish/config.fish")),
        other => Err(format!(
            "unsupported shell {other:?}; add `alias claude=\"ccnotify claude\"` to your shell rc manually"
        )),
    }
}
