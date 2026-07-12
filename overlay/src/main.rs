// The persistent floating companion window for one ccnotify session.
// All communication with the wrapper goes through the Rust side (plain
// HTTP over TcpStream) so the webview never does cross-origin fetches.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod http;

use std::collections::HashMap;
use std::path::PathBuf;

use ccnotify_common::SessionIdentity;
use tauri::{WebviewUrl, WebviewWindowBuilder};

const COMPACT_W: f64 = 250.0;
const COMPACT_H: f64 = 64.0;

#[tauri::command]
fn get_config(identity: tauri::State<'_, SessionIdentity>) -> SessionIdentity {
    identity.inner().clone()
}

/// Long-poll the wrapper for a state snapshot. Blocking IO, so async to
/// keep it off the main thread.
#[tauri::command]
async fn poll_state(
    identity: tauri::State<'_, SessionIdentity>,
    version: u64,
) -> Result<serde_json::Value, String> {
    let path = format!(
        "/overlay/state?version={version}&token={}",
        identity.token
    );
    let body = http::request(identity.port, &identity.token, "GET", &path, None)?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

#[tauri::command]
async fn decide(
    identity: tauri::State<'_, SessionIdentity>,
    id: u64,
    decision: String,
    reason: Option<String>,
) -> Result<(), String> {
    let body = serde_json::json!({ "id": id, "decision": decision, "reason": reason });
    http::request(
        identity.port,
        &identity.token,
        "POST",
        "/overlay/decision",
        Some(&body.to_string()),
    )?;
    Ok(())
}

#[tauri::command]
async fn send_reply(
    identity: tauri::State<'_, SessionIdentity>,
    text: String,
) -> Result<(), String> {
    let body = serde_json::json!({ "text": text });
    http::request(
        identity.port,
        &identity.token,
        "POST",
        "/overlay/reply",
        Some(&body.to_string()),
    )?;
    Ok(())
}

/// Recent (escape-stripped) terminal output for the output view.
#[tauri::command]
async fn get_output(identity: tauri::State<'_, SessionIdentity>) -> Result<String, String> {
    let body = http::request(identity.port, &identity.token, "GET", "/overlay/output", None)?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string())
}

/// Bring the app hosting this session's terminal (VS Code, Terminal,
/// iTerm, ...) to the foreground.
#[tauri::command]
async fn open_host(identity: tauri::State<'_, SessionIdentity>) -> Result<(), String> {
    http::request(identity.port, &identity.token, "POST", "/overlay/open", Some("{}"))?;
    Ok(())
}

/// Ask the wrapper to end the Claude session; the wrapper then closes
/// this overlay as part of its own shutdown.
#[tauri::command]
async fn quit_session(identity: tauri::State<'_, SessionIdentity>) -> Result<(), String> {
    http::request(identity.port, &identity.token, "POST", "/overlay/quit", Some("{}"))?;
    Ok(())
}

/// Close just this overlay (fallback when the wrapper is already gone).
#[tauri::command]
fn exit_overlay() {
    std::process::exit(0);
}

fn main() {
    let identity = match SessionIdentity::from_env() {
        Some(i) => i,
        None => {
            eprintln!("ccnotify-overlay must be launched by the ccnotify wrapper (CCNOTIFY_* env vars missing)");
            std::process::exit(2);
        }
    };

    tauri::Builder::default()
        .manage(identity.clone())
        .invoke_handler(tauri::generate_handler![
            get_config,
            poll_state,
            decide,
            send_reply,
            get_output,
            open_host,
            quit_session,
            exit_overlay
        ])
        .setup(move |app| {
            // Keep the overlay out of the dock/app switcher on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::App("index.html".into()),
            )
            .title(format!("ccnotify — {}", identity.alias))
            .inner_size(COMPACT_W, COMPACT_H)
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .build()?;

            // Restore the last position for this alias; otherwise stagger
            // new overlays so concurrent sessions don't stack.
            let pos = load_position(&identity.alias)
                .unwrap_or_else(|| default_position(&window, identity.port));
            let _ = window.set_position(tauri::PhysicalPosition::new(pos.0, pos.1));

            let alias = identity.alias.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::Moved(p) = event {
                    save_position(&alias, p.x, p.y);
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run ccnotify overlay");
}

fn positions_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".ccnotify").join("positions.json"))
}

fn load_position(alias: &str) -> Option<(i32, i32)> {
    let raw = std::fs::read_to_string(positions_file()?).ok()?;
    let map: HashMap<String, (i32, i32)> = serde_json::from_str(&raw).ok()?;
    map.get(alias).copied()
}

fn save_position(alias: &str, x: i32, y: i32) {
    let Some(path) = positions_file() else { return };
    let mut map: HashMap<String, (i32, i32)> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    map.insert(alias.to_string(), (x, y));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&map) {
        let _ = std::fs::write(&path, json);
    }
}

/// Top-right corner, stepped down-left per session so several overlays
/// land in a visible diagonal instead of stacking.
fn default_position(window: &tauri::WebviewWindow, port: u16) -> (i32, i32) {
    let stagger = (port % 6) as f64;
    let (screen_w, scale) = window
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| (m.size().width as f64, m.scale_factor()))
        .unwrap_or((1440.0, 1.0));
    let x = screen_w - (COMPACT_W + 24.0 + stagger * 40.0) * scale;
    let y = (36.0 + stagger * 76.0) * scale;
    (x.max(0.0) as i32, y as i32)
}
