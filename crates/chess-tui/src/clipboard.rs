//! Copying the game code to the system clipboard: the platform's own tool
//! (pbcopy, wl-copy, xclip, xsel, clip), plus an OSC 52 escape so it also
//! works over SSH in terminals that support it.

use std::io::Write;
use std::process::{Command, Stdio};

use base64::{Engine as _, engine::general_purpose::STANDARD};

/// Puts `text` on the clipboard. Whether it seems to have worked: a tool
/// took it, or OSC 52 went out over SSH (where the tools would reach the
/// wrong machine). Terminals never say whether they honoured OSC 52.
pub fn copy(text: &str) -> bool {
    let osc = osc52(text);
    native(text) || osc
}

/// Sends OSC 52; true if sent over SSH.
fn osc52(text: &str) -> bool {
    // Terminal.app ignores OSC 52; pbcopy covers it.
    if std::env::var("TERM_PROGRAM").as_deref() == Ok("Apple_Terminal") {
        return false;
    }
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", STANDARD.encode(text)).and_then(|_| out.flush()).is_ok()
        && std::env::var_os("SSH_TTY").is_some()
}

fn native(text: &str) -> bool {
    let tools: &[&[&str]] = if cfg!(target_os = "macos") {
        &[&["pbcopy"]]
    } else if cfg!(windows) {
        &[&["clip"]]
    } else {
        &[&["wl-copy"], &["xclip", "-selection", "clipboard"], &["xsel", "--clipboard", "--input"]]
    };
    tools.iter().any(|tool| run(tool, text))
}

fn run(tool: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(tool[0])
        .args(&tool[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    child.wait().is_ok_and(|status| status.success()) && wrote
}
