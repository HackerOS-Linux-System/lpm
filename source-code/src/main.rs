mod alternatives;
mod apt_sources;
mod cache;
mod cli;
mod db;
mod deb;
mod download;
mod fs_install;
mod log;
mod package;
mod solver;
mod ui;

use anyhow::Result;
use cli::{parse_args, Command};

#[tokio::main]
async fn main() {
    // Log every invocation
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
        Command::Install { .. }
            | Command::Remove { .. }
            | Command::Upgrade { .. }
            | Command::Update
            | Command::Clean
            | Command::Autoremove { .. }
    );

    if needs_root && !is_root() {
        anyhow::bail!(
            "This operation requires root privileges.\n  Try: sudo lpm {}",
            std::env::args().skip(1).collect::<Vec<_>>().join(" ")
        );
    }

    match cmd {
        Command::Install { packages, assume_yes, no_recommends } =>
            cli::cmd_install(&packages, assume_yes, no_recommends).await,
        Command::Remove { packages, assume_yes, purge } =>
            cli::cmd_remove(&packages, assume_yes, purge).await,
        Command::Update =>
            cli::cmd_update().await,
        Command::Upgrade { assume_yes } =>
            cli::cmd_upgrade(assume_yes).await,
        Command::Autoremove { assume_yes } =>
            cli::cmd_autoremove(assume_yes).await,
        Command::Search { query, installed } =>
            cli::cmd_search(&query, installed).await,
        Command::Info { package } =>
            cli::cmd_info(&package).await,
        Command::List { installed, upgradeable, available } =>
            cli::cmd_list(installed, upgradeable, available).await,
        Command::Clean =>
            cli::cmd_clean().await,
        Command::History =>
            cli::cmd_history().await,
        Command::Version => { cli::print_version(); Ok(()) }
        Command::Help    => { cli::print_help();    Ok(()) }
    }
}

fn is_root() -> bool {
    #[cfg(unix)]
    { unsafe { libc::getuid() == 0 } }
    #[cfg(not(unix))]
    { true }
}
