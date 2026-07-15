//! Best-effort mirror of Claude Code's own `permissions.allow` / `.deny` /
//! `.defaultMode` config (`.claude/settings.json`, `.claude/settings.local.json`,
//! `~/.claude/settings.json`).
//!
//! The `PreToolUse` hook fires for every tool call before Claude Code
//! consults these rules itself, so without this the overlay ends up
//! asking about things the person already told Claude Code to auto-allow
//! (e.g. a `Bash(git diff:*)` rule saved from a previous "always allow"
//! click). Loaded once per session; under-matching just falls through to
//! the normal overlay ask, so a missed rule is never a correctness risk —
//! only over-matching (auto-allowing something not actually covered)
//! would be, which is why the matchers below stay conservative.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Default)]
pub struct PermissionRules {
    allow: Vec<String>,
    deny: Vec<String>,
    accept_edits: bool,
    bypass: bool,
}

impl PermissionRules {
    /// Merge project (`<cwd>/.claude/settings.local.json`, then
    /// `.../settings.json`) and global (`~/.claude/settings.json`) config.
    pub fn load(cwd: &Path) -> Self {
        let mut rules = PermissionRules::default();
        let mut paths = vec![
            cwd.join(".claude").join("settings.local.json"),
            cwd.join(".claude").join("settings.json"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(home).join(".claude").join("settings.json"));
        }
        for path in paths {
            rules.merge_file(&path);
        }
        rules
    }

    fn merge_file(&mut self, path: &Path) {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            return;
        };
        let Some(perms) = v.get("permissions") else {
            return;
        };
        if let Some(arr) = perms.get("allow").and_then(|a| a.as_array()) {
            self.allow
                .extend(arr.iter().filter_map(|s| s.as_str().map(str::to_string)));
        }
        if let Some(arr) = perms.get("deny").and_then(|a| a.as_array()) {
            self.deny
                .extend(arr.iter().filter_map(|s| s.as_str().map(str::to_string)));
        }
        match perms.get("defaultMode").and_then(|m| m.as_str()) {
            Some("bypassPermissions") => self.bypass = true,
            Some("acceptEdits") => self.accept_edits = true,
            _ => {}
        }
    }

    /// `Some(true)` = auto-allow, `Some(false)` = auto-deny, `None` = no
    /// rule matched — ask via the overlay as before.
    pub fn decide(&self, tool_name: &str, tool_input: &Value) -> Option<bool> {
        if self.deny.iter().any(|r| rule_matches(r, tool_name, tool_input)) {
            return Some(false);
        }
        if self.bypass {
            return Some(true);
        }
        if self.allow.iter().any(|r| rule_matches(r, tool_name, tool_input)) {
            return Some(true);
        }
        if self.accept_edits
            && matches!(tool_name, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
        {
            return Some(true);
        }
        None
    }
}

/// One `permissions.allow`/`.deny` entry, e.g. `Bash(git diff:*)`,
/// `Read(//Users/me/project/**)`, `WebFetch(domain:example.com)`, a bare
/// `WebFetch`, or `*`.
fn rule_matches(rule: &str, tool_name: &str, tool_input: &Value) -> bool {
    let rule = rule.trim();
    if rule == "*" {
        return true;
    }
    let (tool, spec) = match rule.split_once('(') {
        Some((t, rest)) => (t.trim(), rest.strip_suffix(')').unwrap_or(rest).trim()),
        None => (rule, ""),
    };
    if tool != tool_name {
        return false;
    }
    if spec.is_empty() {
        return true; // bare tool name: always matches this tool
    }
    match tool_name {
        "Bash" => tool_input
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|cmd| bash_prefix_matches(spec, cmd)),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => tool_input
            .get("file_path")
            .and_then(|p| p.as_str())
            .is_some_and(|p| path_glob_matches(spec, p)),
        "WebFetch" => tool_input
            .get("url")
            .and_then(|u| u.as_str())
            .is_some_and(|url| webfetch_matches(spec, url)),
        _ => false,
    }
}

/// `cmd` (exact) or `cmd:*` / `cmd *` (prefix, at a word boundary so
/// `git diff:*` doesn't match a literal `git diffx`).
fn bash_prefix_matches(spec: &str, command: &str) -> bool {
    let command = command.trim();
    match spec.strip_suffix(":*").or_else(|| spec.strip_suffix('*')) {
        Some(prefix) => {
            let prefix = prefix.trim_end();
            command == prefix
                || command
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with(char::is_whitespace))
        }
        None => command == spec.trim(),
    }
}

fn webfetch_matches(spec: &str, url: &str) -> bool {
    match spec.strip_prefix("domain:") {
        Some(domain) => {
            let host = url
                .split("://")
                .nth(1)
                .unwrap_or(url)
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("");
            host == domain || host.ends_with(&format!(".{domain}"))
        }
        None => url == spec,
    }
}

/// Minimal glob good enough for the `//abs/path/**` style Claude Code
/// writes into settings: `**` crosses path separators, `*` doesn't.
fn path_glob_matches(spec: &str, path: &str) -> bool {
    let spec = spec.strip_prefix("//").map(|s| format!("/{s}")).unwrap_or_else(|| spec.to_string());
    let spec = match spec.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => format!("{}/{}", home.to_string_lossy(), rest),
            None => spec,
        },
        None => spec,
    };
    glob_match(spec.as_bytes(), path.as_bytes())
}

fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some(b'*') if pattern.get(1) == Some(&b'*') => {
            let rest = pattern[2..].strip_prefix(b"/").unwrap_or(&pattern[2..]);
            (0..=text.len()).any(|i| glob_match(rest, &text[i..]))
        }
        Some(b'*') => {
            let rest = &pattern[1..];
            for i in 0..=text.len() {
                if text[..i].contains(&b'/') {
                    break;
                }
                if glob_match(rest, &text[i..]) {
                    return true;
                }
            }
            false
        }
        Some(&c) => text.first() == Some(&c) && glob_match(&pattern[1..], &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(allow: &[&str], deny: &[&str]) -> PermissionRules {
        PermissionRules {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
            accept_edits: false,
            bypass: false,
        }
    }

    #[test]
    fn bash_exact_and_prefix() {
        let r = rules(&["Bash(cargo --version)", "Bash(git diff:*)"], &[]);
        assert_eq!(r.decide("Bash", &json!({"command": "cargo --version"})), Some(true));
        assert_eq!(r.decide("Bash", &json!({"command": "git diff --stat"})), Some(true));
        assert_eq!(r.decide("Bash", &json!({"command": "git diffx"})), None);
        assert_eq!(r.decide("Bash", &json!({"command": "rm -rf /"})), None);
    }

    #[test]
    fn deny_wins_over_allow() {
        let r = rules(&["Bash(rm:*)"], &["Bash(rm -rf /)"]);
        assert_eq!(r.decide("Bash", &json!({"command": "rm -rf /"})), Some(false));
        assert_eq!(r.decide("Bash", &json!({"command": "rm -f a.txt"})), Some(true));
    }

    #[test]
    fn bare_tool_name_allows_everything_for_that_tool() {
        let r = rules(&["Read"], &[]);
        assert_eq!(r.decide("Read", &json!({"file_path": "/anything"})), Some(true));
        assert_eq!(r.decide("Write", &json!({"file_path": "/anything"})), None);
    }

    #[test]
    fn path_glob() {
        let r = rules(&["Read(//Users/me/project/**)"], &[]);
        assert_eq!(
            r.decide("Read", &json!({"file_path": "/Users/me/project/src/main.rs"})),
            Some(true)
        );
        assert_eq!(r.decide("Read", &json!({"file_path": "/Users/me/other/f.rs"})), None);
    }

    #[test]
    fn webfetch_domain() {
        let r = rules(&["WebFetch(domain:example.com)"], &[]);
        assert_eq!(
            r.decide("WebFetch", &json!({"url": "https://docs.example.com/page"})),
            Some(true)
        );
        assert_eq!(r.decide("WebFetch", &json!({"url": "https://evil.com"})), None);
    }

    #[test]
    fn accept_edits_default_mode() {
        let mut r = rules(&[], &[]);
        r.accept_edits = true;
        assert_eq!(r.decide("Edit", &json!({"file_path": "/x"})), Some(true));
        assert_eq!(r.decide("Bash", &json!({"command": "ls"})), None);
    }
}
