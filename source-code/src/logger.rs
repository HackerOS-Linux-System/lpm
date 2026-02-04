use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use chrono::Local;
use anyhow::Result;

const LOG_DIR: &str = "/tmp/lpm";
const LOG_FILE: &str = "lpm.log";

pub fn init() -> Result<()> {
    if !Path::new(LOG_DIR).exists() {
        fs::create_dir_all(LOG_DIR)?;
    }
    Ok(())
}

pub fn log(level: &str, msg: &str) {
    let path = Path::new(LOG_DIR).join(LOG_FILE);
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_entry = format!("[{}] [{}] {}\n", timestamp, level.to_uppercase(), msg);

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(log_entry.as_bytes());
    }
}

pub fn info(msg: &str) {
    log("INFO", msg);
}

pub fn error(msg: &str) {
    log("ERROR", msg);
}
