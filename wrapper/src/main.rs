mod host;
mod server;
mod session;
mod setup;
mod term;

use std::process::exit;

const USAGE: &str = "\
ccnotify — actionable overlay notifications for Claude Code

Usage:
  ccnotify [--alias <name>] claude [claude args...]   run claude inside the wrapper
  ccnotify setup                                      install hooks + shell alias
  ccnotify uninstall                                  remove hooks + shell alias
";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let mut alias_override: Option<String> = None;
    while args.first().map(String::as_str) == Some("--alias") {
        args.remove(0);
        if args.is_empty() {
            eprintln!("--alias requires a value");
            exit(2);
        }
        alias_override = Some(args.remove(0));
    }

    match args.first().map(String::as_str) {
        Some("setup") => {
            if let Err(e) = setup::setup() {
                eprintln!("ccnotify setup failed: {e}");
                exit(1);
            }
        }
        Some("uninstall") => {
            if let Err(e) = setup::uninstall() {
                eprintln!("ccnotify uninstall failed: {e}");
                exit(1);
            }
        }
        Some("claude") => {
            let claude_args = args[1..].to_vec();
            match session::run(alias_override, claude_args) {
                Ok(code) => exit(code),
                Err(e) => {
                    eprintln!("ccnotify: {e}");
                    exit(1);
                }
            }
        }
        _ => {
            eprint!("{USAGE}");
            exit(2);
        }
    }
}
