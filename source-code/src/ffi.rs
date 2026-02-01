use std::sync::Mutex;
use lazy_static::lazy_static;
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;

lazy_static! {
    static ref GLOBAL_PB: Mutex<Option<ProgressBar>> = Mutex::new(None);
}

pub fn init_progress_bar(len: u64, msg: &str) {
    let pb = ProgressBar::new(len);
    pb.set_style(ProgressStyle::default_bar()
    .template("{spinner:.green} {prefix:.bold.blue} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
    .unwrap()
    .progress_chars("━╸ "));
    pb.set_message(msg.to_string());
    *GLOBAL_PB.lock().unwrap() = Some(pb);
}

pub fn clear_progress_bar() {
    let mut guard = GLOBAL_PB.lock().unwrap();
    if let Some(pb) = guard.take() {
        pb.finish_and_clear();
    }
}

// Callbacks exposed to C++
fn raw_progress_report(percent: f32, msg: String) {
    if let Some(pb) = GLOBAL_PB.lock().unwrap().as_ref() {
        let len = pb.length().unwrap_or(100);
        let pos = (percent / 100.0 * len as f32) as u64;
        pb.set_position(pos);
        pb.set_message(msg);
    }
}

fn raw_phase_report(phase: String) {
    if let Some(pb) = GLOBAL_PB.lock().unwrap().as_ref() {
        pb.set_prefix(phase);
    }
}

#[cxx::bridge(namespace = "legendary")]
pub mod ffi {
    struct PkgInfo {
        name: String,
        section: String,
        version: String,
        current_state: i64,
        size: i64,
    }

    struct PkgDetails {
        name: String,
        version: String,
        section: String,
        maintainer: String,
        description: String,
        installed_size: i64,
        download_size: i64,
        dependencies: Vec<String>,
    }

    struct TransactionSummary {
        to_install: Vec<String>,
        to_remove: Vec<String>,
        to_upgrade: Vec<String>,
    }

    extern "Rust" {
        fn raw_progress_report(percent: f32, msg: String);
        fn raw_phase_report(phase: String);
    }

    unsafe extern "C++" {
        include!("legendary/src/apt_bridge.h");

        type AptClient;

        fn new_apt_client() -> UniquePtr<AptClient>;
        fn init(self: Pin<&mut AptClient>, with_lock: bool);
        fn update_cache(self: Pin<&mut AptClient>);

        fn list_all(self: Pin<&mut AptClient>) -> Vec<PkgInfo>;
        fn search(self: Pin<&mut AptClient>, term: String) -> Vec<PkgInfo>;
        fn find_package(self: Pin<&mut AptClient>, name: String) -> PkgInfo;
        fn get_package_details(self: Pin<&mut AptClient>, name: String) -> PkgDetails;

        fn mark_install(self: Pin<&mut AptClient>, name: String);
        fn mark_remove(self: Pin<&mut AptClient>, name: String);
        fn mark_auto(self: Pin<&mut AptClient>, name: String, is_auto: bool);

        fn mark_upgrade(self: Pin<&mut AptClient>);
        fn mark_full_upgrade(self: Pin<&mut AptClient>);
        fn mark_autoremove(self: Pin<&mut AptClient>);

        fn get_transaction_changes(self: Pin<&mut AptClient>) -> TransactionSummary;

        fn resolve(self: Pin<&mut AptClient>) -> bool;
        fn get_download_size(self: &AptClient) -> i64;
        fn commit_changes(self: Pin<&mut AptClient>) -> bool;
        fn clean_cache(self: Pin<&mut AptClient>);

        fn get_last_error(self: Pin<&mut AptClient>) -> String;
    }
}
