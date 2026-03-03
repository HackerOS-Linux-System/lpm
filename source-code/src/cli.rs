use anyhow::{bail, Result};
use lexopt::prelude::*;
use owo_colors::OwoColorize;

use crate::apt_sources::SourcesList;
use crate::cache::{detect_arch, PackageCache};
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
        _ => bail!("Unexpected argument. Run `lpm help` for usage."),
    };

    match sub.as_str() {
        "install" | "i" | "in" => {
            let mut pkgs  = Vec::new();
            let mut yes   = false;
            let mut norec = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Short('y') | Long("yes") | Long("assumeyes") => yes = true,
                    Long("no-install-recommends") => norec = true,
                    Value(v) => pkgs.push(v.to_string_lossy().to_string()),
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            if pkgs.is_empty() { bail!("No packages specified. Usage: lpm install <pkg...>"); }
            Ok(Command::Install { packages: pkgs, assume_yes: yes, no_recommends: norec })
        }

        "remove" | "rm" | "erase" => {
            let mut pkgs  = Vec::new();
            let mut yes   = false;
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

        "update" | "makecache" => Ok(Command::Update),

        "upgrade" | "dist-upgrade" => {
            let mut yes = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Short('y') | Long("yes") | Long("assumeyes") => yes = true,
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            Ok(Command::Upgrade { assume_yes: yes })
        }

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
            if query.is_empty() { bail!("No search query. Usage: lpm search <query>"); }
            Ok(Command::Search { query, installed })
        }

        "info" | "show" | "information" => {
            let pkg = match parser.next()? {
                Some(Value(v)) => v.to_string_lossy().to_string(),
                _ => bail!("No package specified. Usage: lpm info <package>"),
            };
            Ok(Command::Info { package: pkg })
        }

        "list" | "ls" => {
            let mut installed   = false;
            let mut upgradeable = false;
            let mut available   = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Long("installed")  => installed   = true,
                    Long("upgrades") | Long("upgradeable") => upgradeable = true,
                    Long("available")  => available   = true,
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            Ok(Command::List { installed, upgradeable, available })
        }

        "clean" | "autoclean" => Ok(Command::Clean),
        "history" | "log"     => Ok(Command::History),
        "version" | "--version" | "-V" => Ok(Command::Version),
        "help"    | "--help"    | "-h" => Ok(Command::Help),

        other => bail!("Unknown command: '{}'. Run `lpm help`.", other),
    }
}

// ─────────────────────────────────────────────────────────────
//  update
// ─────────────────────────────────────────────────────────────

pub async fn cmd_update() -> Result<()> {
    let sources = SourcesList::load()?;
    if sources.entries.is_empty() {
        bail!("No repositories configured. Check /etc/apt/sources.list");
    }

    let client = download::HttpClient::new();
    PackageCache::update(&sources, &client).await?;

    println!("{}", "Metadata cache created.".bold());
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  install
// ─────────────────────────────────────────────────────────────

pub async fn cmd_install(names: &[String], assume_yes: bool, no_recommends: bool) -> Result<()> {
    ui::last_metadata_check();

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let arch   = detect_arch();

    let plan = solver.resolve_install(names, no_recommends)?;

    if plan.is_empty() {
        ui::deps_resolved();
        ui::nothing_to_do();
        return Ok(());
    }

    ui::deps_resolved();
    ui::print_transaction_table(&plan, &arch);
    ui::print_transaction_summary(&plan);

    if !assume_yes {
        if !ui::confirm("Is this ok")? {
            println!("{}", "Operation aborted.".bold());
            return Ok(());
        }
    } else {
        println!("{}", "Running with -y, assuming yes.".dimmed());
    }

    // ── Download ──────────────────────────────────────────────
    let all_pkgs: Vec<_> = plan.to_install.iter()
    .chain(plan.to_upgrade.iter())
    .cloned()
    .collect();

    let results = if !all_pkgs.is_empty() {
        println!();
        let client = download::HttpClient::new();
        download::download_packages(&client, &all_pkgs).await?
    } else {
        vec![]
    };

    // ── Install ───────────────────────────────────────────────
    println!();
    ui::print_running_transaction();

    let total = results.len();
    for (i, dl) in results.iter().enumerate() {
        let is_upgrade  = plan.upgrade_from.contains_key(&dl.package.name);
        let old_version = plan.upgrade_from.get(&dl.package.name).cloned();
        let reason      = if names.iter().any(|n| n == &dl.package.name) {
            InstallReason::User
        } else {
            InstallReason::Dependency
        };

        let action = if is_upgrade { "Upgrading" } else { "Installing" };
        let label  = format!("{}-{}.{}", dl.package.name, dl.package.version, dl.package.architecture);
        ui::print_install_step(action, &label, i + 1, total);

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
        install_package(&job, &db)?;
    }

    // Verifying pass (DNF always shows this)
    for (i, dl) in results.iter().enumerate() {
        let label = format!("{}-{}.{}", dl.package.name, dl.package.version, dl.package.architecture);
        ui::print_verify_step(&label, i + 1, total);
    }

    // Summary
    let installed: Vec<_> = plan.to_install.clone();
    let upgraded:  Vec<_> = plan.to_upgrade.clone();
    ui::print_installed_summary(&installed);
    ui::print_upgraded_summary(&upgraded, &plan.upgrade_from);
    println!();
    ui::complete();
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  remove
// ─────────────────────────────────────────────────────────────

pub async fn cmd_remove(names: &[String], assume_yes: bool, purge: bool) -> Result<()> {
    ui::last_metadata_check();

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let arch   = detect_arch();

    let plan = solver.resolve_remove(names)?;

    if plan.is_empty() {
        ui::nothing_to_do();
        return Ok(());
    }

    ui::deps_resolved();
    ui::print_transaction_table(&plan, &arch);
    ui::print_transaction_summary(&plan);

    if purge {
        ui::warn("Config files will also be removed (--purge).");
        println!();
    }

    if !assume_yes {
        if !ui::confirm("Is this ok")? {
            println!("{}", "Operation aborted.".bold());
            return Ok(());
        }
    }

    println!();
    ui::print_running_transaction();

    let total = plan.to_remove.len();
    let mut removed_names = Vec::new();
    for (i, name) in plan.to_remove.iter().enumerate() {
        let inst = match db.get(name) {
            Some(p) => p,
            None    => { ui::warn(&format!("'{}' vanished from DB", name)); continue; }
        };
        let label = format!("{}-{}.{}", inst.name, inst.version, inst.architecture);
        ui::print_remove_step(&label, i + 1, total);
        remove_package(&inst, &db, purge)?;
        removed_names.push(name.clone());
    }

    ui::print_removed_summary(&removed_names);
    println!();
    ui::complete();
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  upgrade
// ─────────────────────────────────────────────────────────────

pub async fn cmd_upgrade(assume_yes: bool) -> Result<()> {
    ui::last_metadata_check();

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let arch   = detect_arch();
    let plan   = solver.resolve_upgrade()?;

    if plan.to_upgrade.is_empty() {
        ui::deps_resolved();
        ui::nothing_to_do();
        return Ok(());
    }

    ui::deps_resolved();
    ui::print_transaction_table(&plan, &arch);
    ui::print_transaction_summary(&plan);

    if !assume_yes {
        if !ui::confirm("Is this ok")? {
            println!("{}", "Operation aborted.".bold());
            return Ok(());
        }
    }

    println!();
    let client  = download::HttpClient::new();
    let results = download::download_packages(&client, &plan.to_upgrade).await?;

    println!();
    ui::print_running_transaction();

    let total = results.len();
    for (i, dl) in results.iter().enumerate() {
        let old_version = plan.upgrade_from.get(&dl.package.name).cloned();
        let label = format!("{}-{}.{}", dl.package.name, dl.package.version, dl.package.architecture);
        ui::print_install_step("Upgrading", &label, i + 1, total);

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
        install_package(&job, &db)?;
    }

    for (i, dl) in results.iter().enumerate() {
        let label = format!("{}-{}.{}", dl.package.name, dl.package.version, dl.package.architecture);
        ui::print_verify_step(&label, i + 1, total);
    }

    ui::print_upgraded_summary(&plan.to_upgrade, &plan.upgrade_from);
    println!();
    ui::complete();
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  autoremove
// ─────────────────────────────────────────────────────────────

pub async fn cmd_autoremove(assume_yes: bool) -> Result<()> {
    ui::last_metadata_check();

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let arch   = detect_arch();
    let plan   = solver.resolve_autoremove()?;

    if plan.to_autoremove.is_empty() {
        ui::nothing_to_do();
        return Ok(());
    }

    ui::deps_resolved();
    ui::print_transaction_table(&plan, &arch);
    ui::print_transaction_summary(&plan);

    if !assume_yes {
        if !ui::confirm("Is this ok")? {
            println!("{}", "Operation aborted.".bold());
            return Ok(());
        }
    }

    println!();
    ui::print_running_transaction();

    let total = plan.to_autoremove.len();
    let mut removed = Vec::new();
    for (i, name) in plan.to_autoremove.iter().enumerate() {
        if let Some(inst) = db.get(name) {
            let label = format!("{}-{}.{}", inst.name, inst.version, inst.architecture);
            ui::print_remove_step(&label, i + 1, total);
            remove_package(&inst, &db, false)?;
            removed.push(name.clone());
        }
    }

    ui::print_removed_summary(&removed);
    println!();
    ui::complete();
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  search
// ─────────────────────────────────────────────────────────────

pub async fn cmd_search(query: &str, installed_only: bool) -> Result<()> {
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;

    let mut results: Vec<_> = cache.search(query);
    if installed_only {
        results.retain(|p| db.is_installed(&p.name));
    }

    if results.is_empty() {
        println!("{}", "No matches found.".bold());
        return Ok(());
    }

    ui::print_search_header(query, results.len());
    for pkg in &results {
        ui::print_search_result(pkg, db.is_installed(&pkg.name));
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  info
// ─────────────────────────────────────────────────────────────

pub async fn cmd_info(package: &str) -> Result<()> {
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;

    // Keep owned copy of the DB version so it lives long enough
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

    let pkg: Option<&crate::package::Package> = cache
    .get(package)
    .or_else(|| from_db.as_ref());

    match pkg {
        None => bail!(
            "No package named '{}' found.\n  Hint: try `lpm search {}`",
            package, package
        ),
        Some(p) => {
            let is_installed  = db.is_installed(package);
            let installed_ver = db.get(package).map(|i| i.version.clone());
            ui::print_package_info(p, is_installed, installed_ver.as_deref());
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

    if upgradeable {
        let upgrades: Vec<_> = db.list_all()?
        .into_iter()
        .filter_map(|p| {
            let avail = cache.get(&p.name)?;
            if crate::package::version_cmp(&avail.version, &p.version)
                == std::cmp::Ordering::Greater
                {
                    Some((p, avail.clone()))
                } else {
                    None
                }
        })
        .collect();

        if upgrades.is_empty() {
            println!("{}", "No packages marked for upgrade.".bold());
        } else {
            for (inst, avail) in &upgrades {
                let repo = avail.repo_base_uri.as_deref()
                .unwrap_or("").trim_end_matches('/').split('/').last().unwrap_or("unknown");
                ui::print_list_entry(
                    &inst.name, &inst.version, &inst.architecture,
                    true, repo, Some(&avail.version),
                );
            }
        }
    } else if installed {
        for p in db.list_all()? {
            let new_ver = cache.get(&p.name).and_then(|a| {
                if crate::package::version_cmp(&a.version, &p.version) == std::cmp::Ordering::Greater {
                    Some(a.version.clone())
                } else {
                    None
                }
            });
            ui::print_list_entry(
                &p.name, &p.version, &p.architecture,
                true, "installed", new_ver.as_deref(),
            );
        }
    } else {
        for p in cache.all_packages() {
            let installed = db.is_installed(&p.name);
            let repo = p.repo_base_uri.as_deref()
            .unwrap_or("").trim_end_matches('/').split('/').last().unwrap_or("unknown");
            ui::print_list_entry(
                &p.name, &p.version, &p.architecture,
                installed, repo, None,
            );
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  clean
// ─────────────────────────────────────────────────────────────

pub async fn cmd_clean() -> Result<()> {
    let dir = std::path::Path::new(download::DL_DIR);
    let mut freed = 0u64;
    let mut count = 0u32;

    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path  = entry.path();
            if path.extension().map_or(false, |e| e == "deb" || e == "part") {
                freed += entry.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(&path).is_ok() { count += 1; }
            }
        }
    }

    println!(
        "{} files removed, {} freed.",
        count.to_string().bold(),
             ui::human_size(freed).yellow().bold()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  history
// ─────────────────────────────────────────────────────────────

pub async fn cmd_history() -> Result<()> {
    let db      = InstalledDb::open()?;
    let entries = db.history(50)?;

    if entries.is_empty() {
        println!("{}", "No transaction history.".bold());
        return Ok(());
    }

    ui::print_history(&entries);
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  version / help
// ─────────────────────────────────────────────────────────────

pub fn print_version() {
    println!(
        "{} {} — Legendary Package Manager",
        "lpm".bold().bright_magenta(),
             env!("CARGO_PKG_VERSION").bold()
    );
    println!("Standalone Debian-compatible. No apt/dpkg required.");
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
    println!("{}", "Package management:".bold().yellow());
    let cmds: &[(&str, &str)] = &[
        ("install <pkg...>",  "Install packages and dependencies"),
        ("remove  <pkg...>",  "Remove packages"),
        ("upgrade",           "Upgrade all installed packages"),
        ("autoremove",        "Remove unneeded dependencies"),
    ];
    for (c, d) in cmds {
        println!("  {:<35} {}", c.cyan(), d.dimmed());
    }
    println!();
    println!("{}", "Repositories and cache:".bold().yellow());
    let cmds2: &[(&str, &str)] = &[
        ("update",            "Refresh package metadata"),
        ("clean",             "Remove cached package files"),
    ];
    for (c, d) in cmds2 {
        println!("  {:<35} {}", c.cyan(), d.dimmed());
    }
    println!();
    println!("{}", "Query:".bold().yellow());
    let cmds3: &[(&str, &str)] = &[
        ("search <query>",             "Search package names and descriptions"),
        ("info   <package>",           "Show package details"),
        ("list [--installed|--upgrades]", "List packages"),
        ("history",                    "Show transaction history"),
    ];
    for (c, d) in cmds3 {
        println!("  {:<35} {}", c.cyan(), d.dimmed());
    }
    println!();
    println!("{}", "Options:".bold().yellow());
    let opts: &[(&str, &str)] = &[
        ("-y, --yes",               "Assume yes"),
        ("--purge",                 "Remove config files too"),
        ("--no-install-recommends", "Skip recommended packages"),
        ("--installed",             "Filter to installed (list/search)"),
        ("--upgrades",              "Show upgradeable only (list)"),
    ];
    for (o, d) in opts {
        println!("  {:<35} {}", o.cyan(), d.dimmed());
    }
    println!();
    println!("{}", "Config:".bold().yellow());
    println!("  {} {}", "Repos:".dimmed(), "/etc/apt/sources.list  or  /etc/lpm/sources-list.toml".cyan());
    println!("  {}  {}", "DB:".dimmed(),   "/var/lib/lpm/lpm.db".cyan());
    println!("  {}   {}", "Cache:".dimmed(), "/var/cache/lpm/archives/".cyan());
    println!();
}
