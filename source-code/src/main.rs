mod alternatives;
mod apt_sources;
mod cache;
mod cli;
mod db;
mod deb;
mod download;
mod dpkg_status;
mod fs_install;
mod import_dpkg;
mod keyring;
mod log;
mod package;
mod repo;
mod solver;
mod solver_sat;
mod ui;

use anyhow::Result;
use cli::{parse_args, Command};

#[tokio::main]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    log::session_start();
    log::cmd(&argv);
    if let Err(e) = run().await {
        let msg = format!("{:#}", e);
        log::error(&msg);
        ui::fatal(&msg);
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cmd = parse_args()?;
    let needs_root = matches!(
        &cmd,
        Command::Install { .. } | Command::Remove { .. } | Command::Upgrade { .. }
        | Command::Update | Command::Clean | Command::Autoremove { .. }
        | Command::Repo { .. } | Command::Key { .. } | Command::ImportDpkg
    );
    if needs_root && !is_root() {
        anyhow::bail!(
            "This operation requires root privileges.\n  Try: sudo lpm {}",
            std::env::args().skip(1).collect::<Vec<_>>().join(" ")
        );
    }
    match cmd {
        Command::Install { packages, assume_yes, with_recommends } =>
        cli::cmd_install(&packages, assume_yes, with_recommends).await,
        Command::Remove { packages, assume_yes, purge } =>
        cli::cmd_remove(&packages, assume_yes, purge).await,
        Command::Update => cli::cmd_update().await,
        Command::Upgrade { assume_yes, only, security } =>
        cli::cmd_upgrade(assume_yes, only, security).await,
        Command::Autoremove { assume_yes } => cli::cmd_autoremove(assume_yes).await,
        Command::Search { query, installed, repo, section, exact, provides } =>
        cli::cmd_search(&query, installed, repo.as_deref(), section.as_deref(), exact, provides.as_deref()).await,
        Command::Info { package } => cli::cmd_info(&package).await,
        Command::List { installed, upgradeable, available } =>
        cli::cmd_list(installed, upgradeable, available).await,
        Command::Clean => cli::cmd_clean().await,
        Command::History { subcmd } => cli::cmd_history(subcmd).await,
        Command::Repo { action } => cli::cmd_repo(action).await,
        Command::Key { action } => cli::cmd_key(action).await,
        Command::WhatProvides { file } => cli::cmd_whatprovides(&file).await,
        Command::Provides { file } => cli::cmd_provides(&file).await,
        Command::CheckUpdate => cli::cmd_check_update().await,
        Command::ImportDpkg => cli::cmd_import_dpkg().await,
        Command::Version => { cli::print_version(); Ok(()) }
        Command::Help => { cli::print_help(); Ok(()) }
    }
}

fn is_root() -> bool {
    #[cfg(unix)]
    { unsafe { libc::getuid() == 0 } }
    #[cfg(not(unix))]
    { true }
}
