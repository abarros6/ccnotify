//! Types shared between the ccnotify wrapper CLI and the overlay app.

use serde::{Deserialize, Serialize};

/// Fixed palette used to derive a session color from its alias.
/// Same alias always hashes to the same color, so no mapping is stored.
pub const PALETTE: &[&str] = &[
    "#e06c75", // red
    "#d19a66", // orange
    "#e5c07b", // yellow
    "#98c379", // green
    "#56b6c2", // teal
    "#61afef", // blue
    "#c678dd", // purple
    "#f28fad", // pink
    "#8ec07c", // sage
    "#83a598", // slate
];

/// Deterministically map an alias to a palette color (FNV-1a hash).
pub fn color_for_alias(alias: &str) -> &'static str {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in alias.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    PALETTE[(hash % PALETTE.len() as u64) as usize]
}

/// Identity of one wrapper session, passed to the overlay via env vars.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIdentity {
    pub alias: String,
    pub color: String,
    pub port: u16,
    pub token: String,
}

impl SessionIdentity {
    /// Read identity from the CCNOTIFY_* environment variables
    /// (set by the wrapper before spawning the overlay).
    pub fn from_env() -> Option<Self> {
        Some(Self {
            alias: std::env::var("CCNOTIFY_ALIAS").ok()?,
            color: std::env::var("CCNOTIFY_COLOR").ok()?,
            port: std::env::var("CCNOTIFY_PORT").ok()?.parse().ok()?,
            token: std::env::var("CCNOTIFY_TOKEN").ok()?,
        })
    }
}

/// Ambient status shown by the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Working,
    NeedsInput,
    Idle,
}

/// A permission request currently blocked on a user decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPermission {
    pub id: u64,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

/// Full state snapshot served to the overlay on each (long-)poll.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayState {
    pub version: u64,
    pub status: Status,
    pub alias: String,
    pub color: String,
    /// Blocked PreToolUse request awaiting allow/deny, if any.
    pub pending: Option<PendingPermission>,
    /// Message to show for Notification/Stop events (last assistant text
    /// or the notification message).
    pub message: Option<String>,
    /// True when a free-text reply typed in the overlay would be accepted
    /// (idle prompt / stop), i.e. Claude is waiting at its input prompt.
    pub can_reply: bool,
}

/// Decision posted back by the overlay for a pending permission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: u64,
    pub decision: String, // "allow" | "deny"
    #[serde(default)]
    pub reason: Option<String>,
}

/// Free-text reply posted by the overlay, written into the pty's stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_is_deterministic() {
        assert_eq!(color_for_alias("api-server"), color_for_alias("api-server"));
        assert!(PALETTE.contains(&color_for_alias("anything")));
    }
}
