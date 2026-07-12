//! Identify and re-focus the application hosting this session's terminal
//! (VS Code, Terminal.app, iTerm, ...), captured from the wrapper's own
//! environment at startup.

use std::path::PathBuf;
use std::process::Command;

/// Bundle-id fragments that identify editor-style hosts. These get the
/// project path passed to `open` so the window holding this project is
/// the one that comes forward ("todesktop" covers Cursor).
#[cfg(target_os = "macos")]
const EDITOR_HINTS: &[&str] = &[
    "vscode",
    "cursor",
    "todesktop",
    "windsurf",
    "zed",
    "sublime",
    "jetbrains",
    "positron",
];

pub struct HostApp {
    bundle_id: Option<String>,
    term_program: Option<String>,
    cwd: Option<PathBuf>,
}

impl HostApp {
    pub fn detect() -> Self {
        Self {
            // Set by macOS for anything launched from a .app bundle —
            // the most precise signal for which GUI app owns this tty.
            bundle_id: std::env::var("__CFBundleIdentifier").ok(),
            term_program: std::env::var("TERM_PROGRAM").ok(),
            cwd: std::env::current_dir().ok(),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn focus(&self) -> Result<(), String> {
        if let Some(id) = &self.bundle_id {
            let mut cmd = Command::new("open");
            cmd.arg("-b").arg(id);
            let id_lower = id.to_lowercase();
            if EDITOR_HINTS.iter().any(|h| id_lower.contains(h)) {
                if let Some(cwd) = &self.cwd {
                    cmd.arg(cwd);
                }
            }
            return run(cmd);
        }
        // No bundle id (e.g. launched via ssh/tmux): fall back to the
        // terminal program name.
        if let Some(tp) = &self.term_program {
            let app = match tp.as_str() {
                "vscode" => "Visual Studio Code",
                "Apple_Terminal" => "Terminal",
                "iTerm.app" => "iTerm",
                "WezTerm" => "WezTerm",
                "ghostty" => "Ghostty",
                "Hyper" => "Hyper",
                other => return Err(format!("don't know how to open terminal {other:?}")),
            };
            let mut cmd = Command::new("open");
            cmd.arg("-a").arg(app);
            if tp == "vscode" {
                if let Some(cwd) = &self.cwd {
                    cmd.arg(cwd);
                }
            }
            return run(cmd);
        }
        Err("could not identify the app hosting this session".into())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn focus(&self) -> Result<(), String> {
        Err("opening the host app is only supported on macOS for now".into())
    }
}

#[cfg(target_os = "macos")]
fn run(mut cmd: Command) -> Result<(), String> {
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("open exited with {s}")),
        Err(e) => Err(format!("failed to run open: {e}")),
    }
}
