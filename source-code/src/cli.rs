use anyhow::{bail, Result};
use lexopt::prelude::*;
use owo_colors::OwoColorize;

use crate::apt_sources::SourcesList;
use crate::cache::PackageCache;
use crate::db::{InstalledDb, InstallReason};
use crate::deb::DebPackage;
use crate::download;
use crate::fs_install::{install_package, remove_package, InstallJob};
use crate::solver::Solver;
use crate::ui;

// ─────────────────────────────────────────────────────────────
//  Command enum
// ─────────────────────────────────────────────────────────────

pub enum Command {
    Install      { packages: Vec<String>, assume_yes: bool, no_recommends: bool },
    Remove       { packages: Vec<String>, assume_yes: bool, purge: bool },
    Update,
    Upgrade      { assume_yes: bool },
    Autoremove   { assume_yes: bool },
    Search       { query: String, installed: bool },
    Info         { package: String },
    List         { installed: bool, upgradeable: bool, available: bool },
    Clean,
    History,
    Version,
    Help,
}

// ─────────────────────────────────────────────────────────────
//  Argument parsing
// ─────────────────────────────────────────────────────────────

pub fn parse_args() -> Result<Command> {
    let mut parser = lexopt::Parser::from_env();

    let sub = match parser.next()? {
        Some(Value(v)) => v.to_string_lossy().to_string(),
        Some(Short('h')) | Some(Long("help"))    => return Ok(Command::Help),
        Some(Short('V')) | Some(Long("version")) => return Ok(Command::Version),
        None => return Ok(Command::Help),
        _other => bail!("Unexpected argument. Run `lpm help` for usage."),
    };

    match sub.as_str() {
        // ── install ───────────────────────────────────────────
        "install" | "i" | "in" => {
            let mut pkgs = Vec::new();
            let mut yes  = false;
            let mut norec = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Short('y') | Long("yes") | Long("assumeyes") => yes = true,
                    Long("no-install-recommends") | Long("setopt=install_weak_deps=False") => norec = true,
                    Value(v) => pkgs.push(v.to_string_lossy().to_string()),
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            if pkgs.is_empty() { bail!("No packages specified. Usage: lpm install <pkg...>"); }
            Ok(Command::Install { packages: pkgs, assume_yes: yes, no_recommends: norec })
        }

        // ── remove ────────────────────────────────────────────
        "remove" | "rm" | "erase" => {
            let mut pkgs = Vec::new();
            let mut yes  = false;
            let mut purge = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Short('y') | Long("yes") | Long("assumeyes") => yes = true,
                    Long("purge") => purge = true,
                    Value(v) => pkgs.push(v.to_string_lossy().to_string()),
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            if pkgs.is_empty() { bail!("No packages specified. Usage: lpm remove <pkg...>"); }
            Ok(Command::Remove { packages: pkgs, assume_yes: yes, purge })
        }

        // ── update ────────────────────────────────────────────
        "update" | "makecache" => Ok(Command::Update),

        // ── upgrade ───────────────────────────────────────────
        "upgrade" | "update-to" | "dist-upgrade" => {
            let mut yes = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Short('y') | Long("yes") | Long("assumeyes") => yes = true,
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            Ok(Command::Upgrade { assume_yes: yes })
        }

        // ── autoremove ────────────────────────────────────────
        "autoremove" | "auto-remove" => {
            let mut yes = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Short('y') | Long("yes") => yes = true,
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            Ok(Command::Autoremove { assume_yes: yes })
        }

        // ── search ────────────────────────────────────────────
        "search" | "se" | "find" => {
            let mut installed = false;
            let mut query     = String::new();
            while let Some(arg) = parser.next()? {
                match arg {
                    Long("installed") => installed = true,
                    Value(v) => {
                        if !query.is_empty() { query.push(' '); }
                        query.push_str(&v.to_string_lossy());
                    }
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            if query.is_empty() { bail!("No search query provided. Usage: lpm search <query>"); }
            Ok(Command::Search { query, installed })
        }

        // ── info ──────────────────────────────────────────────
        "info" | "show" | "information" => {
            let pkg = match parser.next()? {
                Some(Value(v)) => v.to_string_lossy().to_string(),
                _ => bail!("No package specified. Usage: lpm info <package>"),
            };
            Ok(Command::Info { package: pkg })
        }

        // ── list ──────────────────────────────────────────────
        "list" | "ls" => {
            let mut installed   = false;
            let mut upgradeable = false;
            let mut available   = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Long("installed")                 => installed   = true,
                    Long("upgrades") | Long("upgradeable") => upgradeable = true,
                    Long("available")                 => available   = true,
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            Ok(Command::List { installed, upgradeable, available })
        }

        // ── clean ─────────────────────────────────────────────
        "clean" | "autoclean" => Ok(Command::Clean),

        // ── history ───────────────────────────────────────────
        "history" | "log" => Ok(Command::History),

        "version" | "--version" | "-V" => Ok(Command::Version),
        "help"    | "--help"    | "-h" => Ok(Command::Help),

        other => bail!("Unknown command: '{}'. Run `lpm help` for usage.", other),
    }
}

// ─────────────────────────────────────────────────────────────
//  update
// ─────────────────────────────────────────────────────────────

pub async fn cmd_update() -> Result<()> {
    ui::header("Refreshing package index");

    let sources = SourcesList::load()?;
    if sources.entries.is_empty() {
        bail!("No repositories found. Check /etc/apt/sources.list or /etc/lpm/sources-list.toml");
    }

    let client = download::HttpClient::new();
    PackageCache::update(&sources, &client).await?;

    println!();
    ui::ok("Package index is up to date.");
    println!();
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  install
// ─────────────────────────────────────────────────────────────

pub async fn cmd_install(names: &[String], assume_yes: bool, no_recommends: bool) -> Result<()> {
    ui::header(&format!("Resolving dependencies…"));

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);

    let plan = solver.resolve_install(names, no_recommends)?;

    if plan.is_empty() {
        ui::ok("Nothing to do — all requested packages are already installed and up to date.");
        return Ok(());
    }

    ui::print_transaction_summary(&plan);

    if !assume_yes {
        let ok = ui::confirm("Is this ok")?;
        if !ok {
            println!("{}", "  Operation aborted.".yellow());
            return Ok(());
        }
    } else {
        println!("{}", "  Running with -y, assuming yes.".dimmed());
    }

    // ── Download ──────────────────────────────────────────────
    let all_pkgs: Vec<_> = plan.to_install.iter()
    .chain(plan.to_upgrade.iter())
    .cloned()
    .collect();

    if !all_pkgs.is_empty() {
        ui::header("Downloading packages");
        let client = download::HttpClient::new();
        let results = download::download_packages(&client, &all_pkgs).await?;

        // ── Install ───────────────────────────────────────────
        println!();
        ui::header("Installing packages");

        for dl in &results {
            let is_upgrade  = plan.upgrade_from.contains_key(&dl.package.name);
            let old_version = plan.upgrade_from.get(&dl.package.name).cloned();
            let reason      = if names.iter().any(|n| n == &dl.package.name) {
                InstallReason::User
            } else {
                InstallReason::Dependency
            };

            let deb_bytes = std::fs::read(&dl.path)?;
            let deb       = DebPackage::parse(&deb_bytes)?;

            let job = InstallJob {
                pkg: dl.package.clone(),
                deb,
                path: dl.path.clone(),
                reason,
                is_upgrade,
                old_version,
            };

            print!("  {} Installing {}… ", "→".blue().bold(), dl.package.name.bold());
            std::io::Write::flush(&mut std::io::stdout()).ok();

            install_package(&job, &db)?;

            println!("{}", "done".green().bold());
        }
    }

    println!();
    println!("{}", "─".repeat(60).dimmed());
    println!(
        "  {} {} package(s) installed successfully.",
             "Complete!".green().bold(),
             (plan.to_install.len() + plan.to_upgrade.len()).to_string().bold()
    );
    println!("{}", "─".repeat(60).dimmed());
    println!();

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  remove
// ─────────────────────────────────────────────────────────────

pub async fn cmd_remove(names: &[String], assume_yes: bool, purge: bool) -> Result<()> {
    ui::header("Resolving packages to remove");

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);

    let plan = solver.resolve_remove(names)?;

    if plan.is_empty() {
        ui::ok("Nothing to remove.");
        return Ok(());
    }

    ui::print_transaction_summary(&plan);

    if purge {
        ui::warn("Config files will also be removed (--purge).");
    }

    if !assume_yes {
        let ok = ui::confirm("Is this ok")?;
        if !ok {
            println!("{}", "  Operation aborted.".yellow());
            return Ok(());
        }
    }

    ui::header("Removing packages");

    for name in &plan.to_remove {
        let inst = match db.get(name) {
            Some(p) => p,
            None    => { ui::warn(&format!("'{}' not found in DB, skipping", name)); continue; }
        };

        print!("  {} Removing {}… ", "→".blue().bold(), name.bold());
        std::io::Write::flush(&mut std::io::stdout()).ok();

        remove_package(&inst, &db, purge)?;

        println!("{}", "done".green().bold());
    }

    println!();
    println!("{}", "─".repeat(60).dimmed());
    println!(
        "  {} {} package(s) removed.",
             "Complete!".green().bold(),
             plan.to_remove.len().to_string().bold()
    );
    println!("{}", "─".repeat(60).dimmed());
    println!();

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  upgrade
// ─────────────────────────────────────────────────────────────

pub async fn cmd_upgrade(assume_yes: bool) -> Result<()> {
    ui::header("Checking for upgrades");

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let plan   = solver.resolve_upgrade()?;

    if plan.to_upgrade.is_empty() {
        println!();
        ui::ok("System is up to date.");
        println!();
        return Ok(());
    }

    ui::print_transaction_summary(&plan);

    if !assume_yes {
        let ok = ui::confirm("Is this ok")?;
        if !ok {
            println!("{}", "  Operation aborted.".yellow());
            return Ok(());
        }
    }

    ui::header("Downloading upgrade packages");
    let client  = download::HttpClient::new();
    let results = download::download_packages(&client, &plan.to_upgrade).await?;

    println!();
    ui::header("Applying upgrades");

    for dl in &results {
        let old_version = plan.upgrade_from.get(&dl.package.name).cloned();

        let deb_bytes = std::fs::read(&dl.path)?;
        let deb       = DebPackage::parse(&deb_bytes)?;

        let job = InstallJob {
            pkg: dl.package.clone(),
            deb,
            path: dl.path.clone(),
            reason: InstallReason::User,
            is_upgrade: true,
            old_version,
        };

        print!("  {} Upgrading {}… ", "↑".yellow().bold(), dl.package.name.bold());
        std::io::Write::flush(&mut std::io::stdout()).ok();

        install_package(&job, &db)?;

        println!("{}", "done".green().bold());
    }

    println!();
    println!("{}", "─".repeat(60).dimmed());
    println!(
        "  {} {} package(s) upgraded.",
             "Complete!".green().bold(),
             plan.to_upgrade.len().to_string().bold()
    );
    println!("{}", "─".repeat(60).dimmed());
    println!();

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  autoremove
// ─────────────────────────────────────────────────────────────

pub async fn cmd_autoremove(assume_yes: bool) -> Result<()> {
    ui::header("Finding unused packages");

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let plan   = solver.resolve_autoremove()?;

    if plan.to_autoremove.is_empty() {
        println!();
        ui::ok("No unused packages to remove.");
        println!();
        return Ok(());
    }

    ui::print_transaction_summary(&plan);

    if !assume_yes {
        let ok = ui::confirm("Is this ok")?;
        if !ok {
            println!("{}", "  Operation aborted.".yellow());
            return Ok(());
        }
    }

    ui::header("Removing unused packages");

    for name in &plan.to_autoremove {
        let inst = match db.get(name) {
            Some(p) => p,
            None    => continue,
        };

        print!("  {} Removing {}… ", "→".blue().bold(), name.bold());
        std::io::Write::flush(&mut std::io::stdout()).ok();

        remove_package(&inst, &db, false)?;

        println!("{}", "done".green().bold());
    }

    println!();
    ui::ok(&format!(
        "{} unused package(s) removed.",
                    plan.to_autoremove.len()
    ));
    println!();

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  search
// ─────────────────────────────────────────────────────────────

pub async fn cmd_search(query: &str, installed_only: bool) -> Result<()> {
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;

    let results: Vec<_> = if installed_only {
        cache.search(query)
        .into_iter()
        .filter(|p| db.is_installed(&p.name))
        .collect()
    } else {
        cache.search(query)
    };

    if results.is_empty() {
        println!();
        println!("  {} No matches found for '{}'.", "!".yellow().bold(), query.bold());
        println!();
        return Ok(());
    }

    println!();
    println!("  {} {} match(es) found for '{}'",
             "●".cyan().bold(),
             results.len().to_string().bold(),
             query.bold()
    );
    println!();

    ui::print_search_results(&results, &db);
    println!();

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  info
// ─────────────────────────────────────────────────────────────

pub async fn cmd_info(package: &str) -> Result<()> {
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;

    // Build an owned Package from InstalledPackage so lifetimes work out.
    // cache.get() returns &Package (borrowed from cache), but db.get() returns
    // an owned InstalledPackage that we need to convert — we keep it alive here.
    let from_db: Option<crate::package::Package> = db.get(package).map(|inst| {
        crate::package::Package {
            name:              inst.name,
            version:           inst.version,
            architecture:      inst.architecture,
            description_short: inst.description_short,
            section:           inst.section,
            maintainer:        inst.maintainer,
            installed_size_kb: Some(inst.installed_size_kb),
                                                                       depends:           inst.depends,
                                                                       recommends:        inst.recommends,
                                                                       ..Default::default()
        }
    });

    // Prefer live cache entry (has download_size etc.), fall back to installed DB.
    let pkg: Option<&crate::package::Package> = cache
    .get(package)
    .or_else(|| from_db.as_ref());

    match pkg {
        None => {
            bail!("No such package: '{}'\n  Run `lpm search {}` to find available packages.", package, package);
        }
        Some(p) => {
            println!();
            ui::print_package_info(p, &db);
            println!();
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  list
// ─────────────────────────────────────────────────────────────

pub async fn cmd_list(installed: bool, upgradeable: bool, _available: bool) -> Result<()> {
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;

    println!();

    if upgradeable {
        let inst_all = db.list_all()?;
        let upgrades: Vec<_> = inst_all.iter()
        .filter_map(|p| {
            let avail = cache.get(&p.name)?;
            if crate::package::version_cmp(&avail.version, &p.version)
                == std::cmp::Ordering::Greater
                {
                    Some((p, avail))
                } else {
                    None
                }
        })
        .collect();

        if upgrades.is_empty() {
            ui::ok("All packages are up to date.");
        } else {
            println!(
                "  {} {} package(s) can be upgraded.",
                     "●".cyan().bold(),
                     upgrades.len().to_string().bold()
            );
            println!();
            for (inst, avail) in &upgrades {
                ui::print_list_entry(
                    &inst.name,
                    &inst.version,
                    &inst.architecture,
                    true,
                    true,
                    Some(&avail.version),
                );
            }
        }
    } else if installed {
        let pkgs = db.list_all()?;
        println!(
            "  {} {} installed package(s):",
                 "●".cyan().bold(),
                 pkgs.len().to_string().bold()
        );
        println!();
        for p in &pkgs {
            let upgradeable = cache.get(&p.name).map_or(false, |a| {
                crate::package::version_cmp(&a.version, &p.version) == std::cmp::Ordering::Greater
            });
            let new_ver = if upgradeable {
                cache.get(&p.name).map(|a| a.version.clone())
            } else {
                None
            };
            ui::print_list_entry(
                &p.name,
                &p.version,
                &p.architecture,
                true,
                upgradeable,
                new_ver.as_deref(),
            );
        }
    } else {
        let pkgs = cache.all_packages();
        println!(
            "  {} {} packages available in index:",
            "●".cyan().bold(),
                 pkgs.len().to_string().bold()
        );
        println!();
        for p in &pkgs {
            let inst = db.is_installed(&p.name);
            ui::print_list_entry(
                &p.name, &p.version, &p.architecture,
                inst, false, None,
            );
        }
    }

    println!();
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  clean
// ─────────────────────────────────────────────────────────────

pub async fn cmd_clean() -> Result<()> {
    ui::header("Cleaning package cache");

    let dir = std::path::Path::new(download::DL_DIR);
    let mut freed = 0u64;
    let mut count = 0u32;

    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path  = entry.path();
            if path.extension().map_or(false, |e| e == "deb" || e == "part") {
                freed += entry.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(&path).is_ok() {
                    count += 1;
                }
            }
        }
    }

    println!();
    ui::ok(&format!(
        "Removed {} file(s), freed {}.",
                    count,
                    bytesize::ByteSize(freed)
    ));
    println!();

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  history
// ─────────────────────────────────────────────────────────────

pub async fn cmd_history() -> Result<()> {
    let db      = InstalledDb::open()?;
    let entries = db.history(50)?;

    if entries.is_empty() {
        println!();
        ui::info("No transaction history yet.");
        println!();
        return Ok(());
    }

    println!();
    println!(
        "  {} Last {} transaction(s):",
             "●".cyan().bold(),
             entries.len().to_string().bold()
    );
    println!();

    ui::print_history(&entries);
    println!();

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  version / help
// ─────────────────────────────────────────────────────────────

pub fn print_version() {
    println!();
    println!(
        "  {} {} — Legendary Package Manager",
        "lpm".bold().bright_magenta(),
             env!("CARGO_PKG_VERSION").bold()
    );
    println!("  Standalone, dpkg/apt-free, Debian-compatible package manager.");
    println!("  Written in Rust. GPL-3.0.");
    println!();
}

pub fn print_help() {
    println!();
    println!(
        "{}  {}",
        "lpm".bold().bright_magenta(),
             "— Legendary Package Manager".bold()
    );
    println!("  Standalone Debian-compatible package manager. No apt/dpkg required.");
    println!();
    println!("{}", "Usage:".bold().yellow());
    println!("  {} {} [OPTIONS]", "lpm".bold(), "<command>".cyan());
    println!();
    println!("{}", "Package Management:".bold().yellow());

    let cmds: &[(&str, &str)] = &[
        ("install <pkg...>",      "Install packages and their dependencies"),
        ("remove  <pkg...>",      "Remove packages"),
        ("upgrade",               "Upgrade all installed packages"),
        ("autoremove",            "Remove packages no longer required"),
    ];
    for (c, d) in cmds {
        println!("  {:<35} {}", c.cyan(), d.dimmed());
    }

    println!();
    println!("{}", "Index & Cache:".bold().yellow());
    let cmds2: &[(&str, &str)] = &[
        ("update",                "Refresh package index from repositories"),
        ("clean",                 "Remove cached .deb files"),
    ];
    for (c, d) in cmds2 {
        println!("  {:<35} {}", c.cyan(), d.dimmed());
    }

    println!();
    println!("{}", "Query:".bold().yellow());
    let cmds3: &[(&str, &str)] = &[
        ("search  <query>",       "Search for packages by name/description"),
        ("info    <package>",     "Show detailed package information"),
        ("list [--installed|--upgrades]", "List packages"),
        ("history",               "Show transaction history"),
    ];
    for (c, d) in cmds3 {
        println!("  {:<35} {}", c.cyan(), d.dimmed());
    }

    println!();
    println!("{}", "Options:".bold().yellow());
    let opts: &[(&str, &str)] = &[
        ("-y, --yes",                    "Assume yes to all prompts"),
        ("--purge",                      "Also remove config files (remove)"),
        ("--no-install-recommends",      "Skip recommended packages"),
        ("--installed",                  "Filter to installed packages (list/search)"),
        ("--upgrades",                   "Show only upgradeable packages (list)"),
    ];
    for (o, d) in opts {
        println!("  {:<35} {}", o.cyan(), d.dimmed());
    }

    println!();
    println!("{}", "Configuration:".bold().yellow());
    println!("  {}", "Current:  reads /etc/apt/sources.list and /etc/apt/sources.list.d/*.list".dimmed());
    println!("  {}", "Future:   /etc/lpm/sources-list.toml (own format, see README)".dimmed());
    println!("  {}", "Database: /var/lib/lpm/lpm.db (SQLite)".dimmed());
    println!("  {}", "Cache:    /var/cache/lpm/archives/*.deb".dimmed());
    println!();
    println!("{}", "Examples:".bold().yellow());
    println!("  {}  {}",
             "sudo lpm update && sudo lpm upgrade -y".cyan(),
             "# refresh & upgrade all".dimmed()
    );
    println!("  {}  {}",
             "sudo lpm install vim curl git".cyan(),
             "# install packages".dimmed()
    );
    println!("  {}  {}",
             "lpm search \"text editor\"".cyan(),
             "# search available packages".dimmed()
    );
    println!("  {}  {}",
             "lpm list --installed".cyan(),
             "# list installed packages".dimmed()
    );
    println!("  {}  {}",
             "sudo lpm remove snapd --purge".cyan(),
             "# remove + purge config".dimmed()
    );
    println!();
}
