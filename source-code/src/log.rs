//! Simple append-only logger to /tmp/lpm.log
//!
//! Usage:
//!   log::info("Installing vim");
//!   log::warn("dependency foo not found");
//!   log::error("failed to open DB");
//!   log::cmd("install", &["vim", "nano"]);

use std::fs::OpenOptions;
use std::io::Write;

pub const LOG_FILE: &str = "/tmp/lpm.log";

fn write(level: &str, msg: &str) {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("[{}] {:5} {}\n", now, level, msg);

    // Best-effort — never panic if log write fails
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(LOG_FILE) {
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

pub fn error(msg: &str) {
    write("ERROR", msg);
}

/// Log the top-level lpm command invocation.
pub fn cmd(args: &[String]) {
    let line = args.join(" ");
    write("CMD", &format!("lpm {}", line));
}

/// Log start of a transaction.
pub fn transaction_start(action: &str, packages: &[String]) {
    write("INFO", &format!("transaction::{} [{}]", action, packages.join(", ")));
}

/// Log completion of a transaction.
pub fn transaction_done(action: &str, packages: &[String]) {
    write("INFO", &format!("transaction::{} done [{}]", action, packages.join(", ")));
}

/// Log a single package operation.
pub fn pkg(action: &str, name: &str, version: &str) {
    write("PKG", &format!("{:<10} {}-{}", action, name, version));
}

/// Log a file operation during install/remove.
pub fn file_op(action: &str, path: &str) {
    write("FILE", &format!("{:<8} {}", action, path));
}

/// Log separator line (start of lpm session).
pub fn session_start() {
    let now   = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let uname = std::process::Command::new("uname").arg("-r")
    .output().ok()
    .and_then(|o| String::from_utf8(o.stdout).ok())
    .unwrap_or_default();
    let uname = uname.trim();

    let line = format!(
        "\n──────────────────────────────────────────────────────────────────\n\
[{now}] SESSION START  pid={}  kernel={uname}\n\
──────────────────────────────────────────────────────────────────\n",
std::process::id()
    );

    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(LOG_FILE) {
        let _ = f.write_all(line.as_bytes());
    }
}
