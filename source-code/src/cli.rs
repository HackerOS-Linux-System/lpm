use anyhow::{bail, Context, Result};
use lexopt::prelude::*;
use owo_colors::OwoColorize;

use crate::apt_sources::SourcesList;
use crate::cache::{detect_arch, PackageCache};
use crate::db::{InstalledDb, InstallReason};
use crate::deb::DebPackage;
use crate::download;
use crate::fs_install::{install_package, remove_package, InstallJob};
use crate::log;
use crate::solver::Solver;
use crate::ui;
use crate::keyring;
use crate::repo;

// ─────────────────────────────────────────────────────────────
//  Command enum
// ─────────────────────────────────────────────────────────────

pub enum Command {
    Install {
        packages: Vec<String>,
        assume_yes: bool,
        with_recommends: bool,
    },
    Remove {
        packages: Vec<String>,
        assume_yes: bool,
        purge: bool,
    },
    Update,
    Upgrade {
        assume_yes: bool,
        only: Option<String>,
        security: bool,
    },
    Autoremove {
        assume_yes: bool,
    },
    Search {
        query: String,
        installed: bool,
        repo: Option<String>,
        section: Option<String>,
        exact: bool,
        provides: Option<String>,
    },
    Info {
        package: String,
    },
    List {
        installed: bool,
        upgradeable: bool,
        available: bool,
    },
    Clean,
    History {
        subcmd: Option<HistorySubcmd>,
    },
    Repo {
        action: RepoAction,
    },
    Key {
        action: KeyAction,
    },
    WhatProvides { file: String },
    Provides { file: String },
    CheckUpdate,
    ImportDpkg,
    Version,
    Help,
}

pub enum RepoAction {
    List,
    Add {
        uri: String,
        suite: String,
        components: Vec<String>,
    },
    Remove { id: usize },
    Enable { id: usize },
    Disable { id: usize },
}

pub enum KeyAction {
    Add { path: String },
    List,
}

pub enum HistorySubcmd {
    Undo { id: i64 },
    Redo { id: i64 },
    Diff { id1: i64, id2: i64 },
    Export { path: String },
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
        _    => bail!("Unexpected argument. Run `lpm help` for usage."),
    };

    match sub.as_str() {
        "install" | "i" | "in" => {
            let mut pkgs = Vec::new();
            let mut yes  = false;
            let mut with_rec = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Short('y') | Long("yes") | Long("assumeyes") => yes = true,
                    Long("with-recommends") | Long("install-recommends") => with_rec = true,
                    Long("no-install-recommends") => with_rec = false,
                    Value(v) => pkgs.push(v.to_string_lossy().to_string()),
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            if pkgs.is_empty() {
                bail!("No packages specified. Usage: lpm install <pkg...>");
            }
            Ok(Command::Install { packages: pkgs, assume_yes: yes, with_recommends: with_rec })
        }

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
            if pkgs.is_empty() {
                bail!("No packages specified. Usage: lpm remove <pkg...>");
            }
            Ok(Command::Remove { packages: pkgs, assume_yes: yes, purge })
        }

        "update" | "makecache" => Ok(Command::Update),

        "upgrade" | "dist-upgrade" => {
            let mut yes  = false;
            let mut only = None;
            let mut security = false;
            while let Some(arg) = parser.next()? {
                match arg {
                    Short('y') | Long("yes") => yes = true,
                    Long("only") => {
                        only = Some(parser.value()?.to_string_lossy().to_string());
                    }
                    Long("security") => security = true,
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            Ok(Command::Upgrade { assume_yes: yes, only, security })
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
            let mut repo      = None;
            let mut section   = None;
            let mut exact     = false;
            let mut provides  = None;
            let mut query     = String::new();
            while let Some(arg) = parser.next()? {
                match arg {
                    Long("installed") => installed = true,
                    Long("repo") => repo = Some(parser.value()?.to_string_lossy().to_string()),
                    Long("section") => section = Some(parser.value()?.to_string_lossy().to_string()),
                    Long("exact") => exact = true,
                    Long("provides") => provides = Some(parser.value()?.to_string_lossy().to_string()),
                    Value(v) => {
                        if !query.is_empty() { query.push(' '); }
                        query.push_str(&v.to_string_lossy());
                    }
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            if query.is_empty() {
                bail!("No search query. Usage: lpm search <query>");
            }
            Ok(Command::Search { query, installed, repo, section, exact, provides })
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
                    Long("installed") => installed = true,
                    Long("upgrades") | Long("upgradeable") => upgradeable = true,
                    Long("available") => available = true,
                    _ => bail!("Unknown flag: {}", arg.unexpected()),
                }
            }
            Ok(Command::List { installed, upgradeable, available })
        }

        "clean" | "autoclean" => Ok(Command::Clean),

        "history" | "log" => {
            let mut subcmd = None;
            if let Some(arg) = parser.next()? {
                match arg {
                    Value(v) => {
                        let val = v.to_string_lossy().to_string();
                        match val.as_ref() {
                            "undo" => {
                                let id = parser.value()?.to_string_lossy().parse()
                                .map_err(|_| anyhow::anyhow!("Invalid ID"))?;
                                subcmd = Some(HistorySubcmd::Undo { id });
                            }
                            "redo" => {
                                let id = parser.value()?.to_string_lossy().parse()
                                .map_err(|_| anyhow::anyhow!("Invalid ID"))?;
                                subcmd = Some(HistorySubcmd::Redo { id });
                            }
                            "diff" => {
                                let id1 = parser.value()?.to_string_lossy().parse()
                                .map_err(|_| anyhow::anyhow!("Invalid ID1"))?;
                                let id2 = parser.value()?.to_string_lossy().parse()
                                .map_err(|_| anyhow::anyhow!("Invalid ID2"))?;
                                subcmd = Some(HistorySubcmd::Diff { id1, id2 });
                            }
                            "export" => {
                                let path = parser.value()?.to_string_lossy().to_string();
                                subcmd = Some(HistorySubcmd::Export { path });
                            }
                            _ => bail!("Unknown history subcommand: {}", val),
                        }
                    }
                    _ => {}
                }
            }
            Ok(Command::History { subcmd })
        }

        "repo" => {
            let mut action = RepoAction::List;
            if let Some(arg) = parser.next()? {
                match arg {
                    Value(v) => {
                        let val = v.to_string_lossy().to_string();
                        match val.as_ref() {
                            "list" => {}
                            "add" => {
                                let uri   = parser.value()?.to_string_lossy().to_string();
                                let suite = parser.value()?.to_string_lossy().to_string();
                                let mut components = Vec::new();
                                while let Ok(Some(Value(c))) = parser.next() {
                                    components.push(c.to_string_lossy().to_string());
                                }
                                if components.is_empty() {
                                    bail!("No components given for repo add");
                                }
                                action = RepoAction::Add { uri, suite, components };
                            }
                            "remove" => {
                                let id = parser.value()?.to_string_lossy().parse()
                                .map_err(|_| anyhow::anyhow!("Invalid ID"))?;
                                action = RepoAction::Remove { id };
                            }
                            "enable" => {
                                let id = parser.value()?.to_string_lossy().parse()
                                .map_err(|_| anyhow::anyhow!("Invalid ID"))?;
                                action = RepoAction::Enable { id };
                            }
                            "disable" => {
                                let id = parser.value()?.to_string_lossy().parse()
                                .map_err(|_| anyhow::anyhow!("Invalid ID"))?;
                                action = RepoAction::Disable { id };
                            }
                            _ => bail!("Unknown repo subcommand: {}", val),
                        }
                    }
                    _ => {}
                }
            }
            Ok(Command::Repo { action })
        }

        "key" => {
            let mut action = KeyAction::List;
            if let Some(arg) = parser.next()? {
                match arg {
                    Value(v) => {
                        let val = v.to_string_lossy().to_string();
                        match val.as_ref() {
                            "list" => {}
                            "add"  => {
                                let path = parser.value()?.to_string_lossy().to_string();
                                action = KeyAction::Add { path };
                            }
                            _ => bail!("Unknown key subcommand: {}", val),
                        }
                    }
                    _ => {}
                }
            }
            Ok(Command::Key { action })
        }

        "whatprovides" => {
            let file = match parser.next()? {
                Some(Value(v)) => v.to_string_lossy().to_string(),
                _ => bail!("No file specified. Usage: lpm whatprovides <file>"),
            };
            Ok(Command::WhatProvides { file })
        }

        "provides" => {
            let file = match parser.next()? {
                Some(Value(v)) => v.to_string_lossy().to_string(),
                _ => bail!("No file specified. Usage: lpm provides <file>"),
            };
            Ok(Command::Provides { file })
        }

        "check-update" => Ok(Command::CheckUpdate),
        "import-dpkg"  => Ok(Command::ImportDpkg),
        "version" | "--version" | "-V" => Ok(Command::Version),
        "help"    | "--help"    | "-h" => Ok(Command::Help),

        other => bail!("Unknown command: '{}'. Run `lpm help`.", other),
    }
}

// ─────────────────────────────────────────────────────────────
//  Command implementations
// ─────────────────────────────────────────────────────────────

pub async fn cmd_update() -> Result<()> {
    log::info("command: update");
    let sources = SourcesList::load()?;
    if sources.entries.is_empty() {
        bail!(
            "No repositories configured.\n\
Check /etc/lpm/sources-list.toml or /etc/lpm/sources.list"
        );
    }
    let client = download::HttpClient::new();
    PackageCache::update(&sources, &client).await?;
    log::info("update complete");
    println!("{}", "Metadata cache created.".bold());
    Ok(())
}

pub async fn cmd_install(
    names: &[String],
    assume_yes: bool,
    with_recommends: bool,
) -> Result<()> {
    log::transaction_start("install", names);
    ui::last_metadata_check();

    let cache = PackageCache::load()?;
    if cache.len() == 0 {
        bail!("Package cache is empty. Run `lpm update` first.");
    }

    let db = InstalledDb::open()?;

    // Użyj SAT solver jeśli skompilowany, inaczej greedy
    let plan = if cfg!(feature = "sat-solver") {
        crate::solver_sat::resolve_with_sat(&cache, &db, names, !with_recommends)?
    } else {
        let solver = Solver::new(&cache, &db);
        solver.resolve_install(names, !with_recommends)?
    };

    if plan.is_empty() {
        ui::deps_resolved();
        ui::nothing_to_do();
        log::info("install: nothing to do");
        return Ok(());
    }

    let arch = detect_arch();
    let install_names: Vec<String> = plan.to_install.iter().map(|p| p.name.clone()).collect();
    let upgrade_names: Vec<String> = plan.to_upgrade.iter().map(|p| p.name.clone()).collect();
    log::info(&format!(
        "plan: install={:?} upgrade={:?}",
        install_names, upgrade_names
    ));

    ui::deps_resolved();
    ui::print_transaction_table(&plan, &arch);
    ui::print_transaction_summary(&plan);

    if !assume_yes && !ui::confirm("Is this ok")? {
        log::info("install: aborted by user");
        println!("{}", "Operation aborted.".bold());
        return Ok(());
    } else if assume_yes {
        println!("{}", "Running with -y, assuming yes.".dimmed());
    }

    let all_pkgs: Vec<_> = plan
    .to_install
    .iter()
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

    println!();
    ui::print_running_transaction();

    let total = results.len();
    for (i, dl) in results.iter().enumerate() {
        let is_upgrade  = plan.upgrade_from.contains_key(&dl.package.name);
        let old_version = plan.upgrade_from.get(&dl.package.name).cloned();
        let reason = if names.iter().any(|n| n == &dl.package.name) {
            InstallReason::User
        } else {
            InstallReason::Dependency
        };

        let action = if is_upgrade { "Upgrading" } else { "Installing" };
        let label  = format!(
            "{}-{}.{}",
            dl.package.name, dl.package.version, dl.package.architecture
        );
        ui::print_install_step(action, &label, i + 1, total);

        let deb_bytes = std::fs::read(&dl.path)?;
        let deb = DebPackage::parse(&deb_bytes)?;
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

    for (i, dl) in results.iter().enumerate() {
        let label = format!(
            "{}-{}.{}",
            dl.package.name, dl.package.version, dl.package.architecture
        );
        ui::print_verify_step(&label, i + 1, total);
    }

    ui::print_installed_summary(&plan.to_install);
    ui::print_upgraded_summary(&plan.to_upgrade, &plan.upgrade_from);
    println!();
    ui::complete();
    log::transaction_done("install", names);
    Ok(())
}

pub async fn cmd_remove(names: &[String], assume_yes: bool, purge: bool) -> Result<()> {
    log::transaction_start("remove", names);
    ui::last_metadata_check();

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let arch   = detect_arch();
    let plan   = solver.resolve_remove(names)?;

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

    if !assume_yes && !ui::confirm("Is this ok")? {
        log::info("remove: aborted by user");
        println!("{}", "Operation aborted.".bold());
        return Ok(());
    }

    println!();
    ui::print_running_transaction();

    let total = plan.to_remove.len();
    let mut removed_names = Vec::new();
    for (i, name) in plan.to_remove.iter().enumerate() {
        let inst = match db.get(name) {
            Some(p) => p,
            None    => {
                ui::warn(&format!("'{}' vanished from DB", name));
                continue;
            }
        };
        let label = format!(
            "{}-{}.{}",
            inst.name, inst.version, inst.architecture
        );
        ui::print_remove_step(&label, i + 1, total);
        remove_package(&inst, &db, purge)?;
        removed_names.push(name.clone());
    }

    ui::print_removed_summary(&removed_names);
    println!();
    ui::complete();
    log::transaction_done("remove", names);
    Ok(())
}

pub async fn cmd_upgrade(assume_yes: bool, only: Option<String>, security: bool) -> Result<()> {
    log::info("command: upgrade");
    ui::last_metadata_check();

    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let arch   = detect_arch();
    let mut plan = solver.resolve_upgrade()?;

    if let Some(pkg_name) = only {
        plan.to_upgrade.retain(|p| p.name == pkg_name);
        if plan.to_upgrade.is_empty() {
            println!("No upgrade for package '{}'", pkg_name);
            return Ok(());
        }
    }

    if security {
        let sources = SourcesList::load()?;
        let security_repos: Vec<_> = sources
        .entries
        .iter()
        .filter(|e| e.suite.contains("security"))
        .map(|e| &e.uri)
        .collect();
        plan.to_upgrade.retain(|p| {
            p.repo_base_uri
            .as_deref()
            .map(|uri| security_repos.iter().any(|r| uri.contains(r.as_str())))
            .unwrap_or(false)
        });
        if plan.to_upgrade.is_empty() {
            println!("No security updates available.");
            return Ok(());
        }
    }

    if plan.to_upgrade.is_empty() {
        ui::deps_resolved();
        ui::nothing_to_do();
        return Ok(());
    }

    ui::deps_resolved();
    ui::print_transaction_table(&plan, &arch);
    ui::print_transaction_summary(&plan);

    if !assume_yes && !ui::confirm("Is this ok")? {
        log::info("upgrade: aborted by user");
        println!("{}", "Operation aborted.".bold());
        return Ok(());
    }

    println!();
    let client  = download::HttpClient::new();
    let results = download::download_packages(&client, &plan.to_upgrade).await?;

    println!();
    ui::print_running_transaction();

    let total = results.len();
    for (i, dl) in results.iter().enumerate() {
        let old_version = plan.upgrade_from.get(&dl.package.name).cloned();
        let label = format!(
            "{}-{}.{}",
            dl.package.name, dl.package.version, dl.package.architecture
        );
        ui::print_install_step("Upgrading", &label, i + 1, total);

        let deb_bytes = std::fs::read(&dl.path)?;
        let deb = DebPackage::parse(&deb_bytes)?;
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
        let label = format!(
            "{}-{}.{}",
            dl.package.name, dl.package.version, dl.package.architecture
        );
        ui::print_verify_step(&label, i + 1, total);
    }

    ui::print_upgraded_summary(&plan.to_upgrade, &plan.upgrade_from);
    println!();
    ui::complete();
    log::info("upgrade complete");
    Ok(())
}

pub async fn cmd_autoremove(assume_yes: bool) -> Result<()> {
    log::info("command: autoremove");
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

    if !assume_yes && !ui::confirm("Is this ok")? {
        log::info("autoremove: aborted");
        println!("{}", "Operation aborted.".bold());
        return Ok(());
    }

    println!();
    ui::print_running_transaction();

    let total = plan.to_autoremove.len();
    let mut removed = Vec::new();
    for (i, name) in plan.to_autoremove.iter().enumerate() {
        if let Some(inst) = db.get(name) {
            let label = format!(
                "{}-{}.{}",
                inst.name, inst.version, inst.architecture
            );
            ui::print_remove_step(&label, i + 1, total);
            remove_package(&inst, &db, false)?;
            removed.push(name.clone());
        }
    }

    ui::print_removed_summary(&removed);
    println!();
    ui::complete();
    log::transaction_done("autoremove", &removed);
    Ok(())
}

pub async fn cmd_search(
    query: &str,
    installed: bool,
    repo: Option<&str>,
    section: Option<&str>,
    exact: bool,
    provides: Option<&str>,
) -> Result<()> {
    log::info(&format!("search: {}", query));
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;

    let mut results: Vec<_> = cache.search(query);
    if installed {
        results.retain(|p| db.is_installed(&p.name));
    }
    if let Some(repo_name) = repo {
        results.retain(|p| {
            p.repo_base_uri.as_deref().unwrap_or("").contains(repo_name)
        });
    }
    if let Some(sec) = section {
        results.retain(|p| p.section.as_deref() == Some(sec));
    }
    if exact {
        results.retain(|p| p.name.to_lowercase() == query.to_lowercase());
    }
    if let Some(prov) = provides {
        results.retain(|p| {
            p.provides.as_deref().map_or(false, |s| s.contains(prov))
            || p.filename.as_deref().map_or(false, |f| f.contains(prov))
        });
    }

    if results.is_empty() {
        println!("{}", "No matches found.".bold());
        return Ok(());
    }

    ui::print_search_header(query, results.len());
    for pkg in results {
        ui::print_search_result(pkg, db.is_installed(&pkg.name));
    }
    Ok(())
}

pub async fn cmd_info(package: &str) -> Result<()> {
    log::info(&format!("info: {}", package));
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;

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

    let pkg: Option<&crate::package::Package> =
    cache.get(package).or_else(|| from_db.as_ref());

    match pkg {
        None => bail!(
            "No package named '{}' found.\n  Hint: try `lpm search {}`",
            package, package
        ),
        Some(p) => {
            let is_installed = db.is_installed(package);
            let installed_ver = db.get(package).map(|i| i.version.clone());
            ui::print_package_info(p, is_installed, installed_ver.as_deref());
        }
    }
    Ok(())
}

pub async fn cmd_list(installed: bool, upgradeable: bool, _available: bool) -> Result<()> {
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;

    if upgradeable {
        let upgrades: Vec<_> = db
        .list_all()?
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
                let repo = avail
                .repo_base_uri
                .as_deref()
                .unwrap_or("")
                .trim_end_matches('/')
                .split('/')
                .last()
                .unwrap_or("unknown");
                ui::print_list_entry(
                    &inst.name, &inst.version, &inst.architecture,
                    true, repo, Some(&avail.version),
                );
            }
        }
    } else if installed {
        for p in db.list_all()? {
            let new_ver = cache.get(&p.name).and_then(|a| {
                if crate::package::version_cmp(&a.version, &p.version)
                    == std::cmp::Ordering::Greater
                    {
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
            let is_inst = db.is_installed(&p.name);
            let repo = p
            .repo_base_uri
            .as_deref()
            .unwrap_or("")
            .trim_end_matches('/')
            .split('/')
            .last()
            .unwrap_or("unknown");
            ui::print_list_entry(
                &p.name, &p.version, &p.architecture,
                is_inst, repo, None,
            );
        }
    }
    Ok(())
}

pub async fn cmd_clean() -> Result<()> {
    log::info("command: clean");
    let dir    = std::path::Path::new(download::DL_DIR);
    let mut freed = 0u64;
    let mut count = 0u32;

    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path  = entry.path();
            if path.extension().map_or(false, |e| e == "deb" || e == "part") {
                freed += entry.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(&path).is_ok() {
                    log::file_op("clean", &path.to_string_lossy());
                    count += 1;
                }
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

pub async fn cmd_history(subcmd: Option<HistorySubcmd>) -> Result<()> {
    let db = InstalledDb::open()?;

    match subcmd {
        None => {
            let entries = db.history(50)?;
            if entries.is_empty() {
                println!("{}", "No transaction history.".bold());
                return Ok(());
            }
            ui::print_history(&entries);
        }
        Some(HistorySubcmd::Undo { id }) => {
            let entry = db
            .get_history_entry(id)
            .context("No such history entry")?
            .context("History entry not found")?;

            match entry.action.as_str() {
                "install" => {
                    let inst = db
                    .get(&entry.package)
                    .context("Package not installed, cannot undo install")?;
                    remove_package(&inst, &db, false)?;
                    println!("Undone install of {}", entry.package);
                }
                "remove" => {
                    let cache  = PackageCache::load()?;
                    let solver = Solver::new(&cache, &db);
                    let plan   = solver.resolve_install(&[entry.package.clone()], true)?;
                    let client = download::HttpClient::new();
                    let results = download::download_packages(&client, &plan.to_install).await?;
                    for dl in &results {
                        let deb_bytes = std::fs::read(&dl.path)?;
                        let deb = DebPackage::parse(&deb_bytes)?;
                        install_package(
                            &InstallJob {
                                pkg: dl.package.clone(), deb,
                                        path: dl.path.clone(),
                                        reason: InstallReason::User,
                                        is_upgrade: false, old_version: None,
                            },
                            &db,
                        )?;
                    }
                    println!("Reinstalled {}", entry.package);
                }
                "upgrade" => {
                    let old_ver = entry.old_ver.context("No old version for upgrade")?;
                    let cache = PackageCache::load()?;
                    let pkg = cache
                    .get_exact(&entry.package, &old_ver, &detect_arch())
                    .context("Old version not found in cache, cannot downgrade")?;
                    let client  = download::HttpClient::new();
                    let results = download::download_packages(&client, &[pkg.clone()]).await?;
                    for dl in &results {
                        let deb_bytes = std::fs::read(&dl.path)?;
                        let deb = DebPackage::parse(&deb_bytes)?;
                        install_package(
                            &InstallJob {
                                pkg: dl.package.clone(), deb,
                                        path: dl.path.clone(),
                                        reason: InstallReason::User,
                                        is_upgrade: false, old_version: None,
                            },
                            &db,
                        )?;
                    }
                    println!("Downgraded {} to {}", entry.package, old_ver);
                }
                _ => bail!("Unsupported action for undo: {}", entry.action),
            }
        }
        Some(HistorySubcmd::Redo { id }) => {
            let entry = db
            .get_history_entry(id)
            .context("No such history entry")?
            .context("History entry not found")?;
            match entry.action.as_str() {
                "install" => {
                    let cache = PackageCache::load()?;
                    let pkg = cache.get(&entry.package).context("Package not found in cache")?;
                    let client  = download::HttpClient::new();
                    let results = download::download_packages(&client, &[pkg.clone()]).await?;
                    for dl in &results {
                        let deb_bytes = std::fs::read(&dl.path)?;
                        let deb = DebPackage::parse(&deb_bytes)?;
                        install_package(
                            &InstallJob {
                                pkg: dl.package.clone(), deb,
                                        path: dl.path.clone(),
                                        reason: InstallReason::User,
                                        is_upgrade: false, old_version: None,
                            },
                            &db,
                        )?;
                    }
                    println!("Redone install of {}", entry.package);
                }
                "remove" => {
                    let inst = db
                    .get(&entry.package)
                    .context("Package not installed, cannot redo remove")?;
                    remove_package(&inst, &db, false)?;
                    println!("Redone remove of {}", entry.package);
                }
                "upgrade" => {
                    let new_ver = entry.new_ver.context("No new version for upgrade")?;
                    let cache = PackageCache::load()?;
                    let pkg = cache
                    .get_exact(&entry.package, &new_ver, &detect_arch())
                    .context("New version not found in cache")?;
                    let client  = download::HttpClient::new();
                    let results = download::download_packages(&client, &[pkg.clone()]).await?;
                    for dl in &results {
                        let deb_bytes = std::fs::read(&dl.path)?;
                        let deb = DebPackage::parse(&deb_bytes)?;
                        install_package(
                            &InstallJob {
                                pkg: dl.package.clone(), deb,
                                        path: dl.path.clone(),
                                        reason: InstallReason::User,
                                        is_upgrade: true,
                                        old_version: entry.old_ver.clone(),
                            },
                            &db,
                        )?;
                    }
                    println!("Redone upgrade of {} to {}", entry.package, new_ver);
                }
                _ => bail!("Unsupported action for redo: {}", entry.action),
            }
        }
        Some(HistorySubcmd::Diff { id1, id2 }) => {
            let e1 = db.get_history_entry(id1)?.context("History entry 1 not found")?;
            let e2 = db.get_history_entry(id2)?.context("History entry 2 not found")?;
            println!("Diff between transaction {} and {}:", id1, id2);
            println!("  {}: {} {} {}", e1.id, e1.action, e1.package, e1.timestamp);
            println!("  {}: {} {} {}", e2.id, e2.action, e2.package, e2.timestamp);
            println!("(Detailed diff not implemented)");
        }
        Some(HistorySubcmd::Export { path }) => {
            let entries = db.history(1000)?;
            let json = serde_json::to_string_pretty(&entries)?;
            std::fs::write(&path, json)?;
            println!("History exported to {}", path);
        }
    }
    Ok(())
}

pub async fn cmd_repo(action: RepoAction) -> Result<()> {
    match action {
        RepoAction::List => {
            let repos = repo::RepoManager::list()?;
            if repos.is_empty() {
                println!("No repositories configured.");
                println!("Create /etc/lpm/sources-list.toml or /etc/lpm/sources.list");
                return Ok(());
            }
            println!("{}", "Configured repositories:".bold());
            println!("{}", "─".repeat(70).dimmed());
            for (i, entry) in &repos {
                let status = if entry.enabled {
                    "enabled".green().bold().to_string()
                } else {
                    "disabled".red().to_string()
                };
                let name = entry.label.as_deref().unwrap_or("(unnamed)");
                println!(
                    "{}: {} [{}]",
                    i.to_string().cyan().bold(), name.bold(), status
                );
                println!("   URI:   {}", entry.uri.dimmed());
                println!(
                    "   Suite: {}  Components: {}",
                    entry.suite.cyan(),
                         entry.components.join(", ").dimmed()
                );
                if !entry.arches.is_empty() {
                    println!("   Arch:  {}", entry.arches.join(", ").dimmed());
                }
                println!();
            }
        }
        RepoAction::Add { uri, suite, components } => {
            repo::RepoManager::add(&uri, &suite, &components)?;
        }
        RepoAction::Remove { id } => {
            repo::RepoManager::remove(id)?;
            println!("Repository {} removed.", id);
        }
        RepoAction::Enable { id } => {
            repo::RepoManager::enable(id)?;
            println!("Repository {} enabled.", id);
        }
        RepoAction::Disable { id } => {
            repo::RepoManager::disable(id)?;
            println!("Repository {} disabled.", id);
        }
    }
    Ok(())
}

pub async fn cmd_key(action: KeyAction) -> Result<()> {
    match action {
        KeyAction::List => {
            let keys = keyring::Keyring::list()?;
            if keys.is_empty() {
                println!("No keys in keyring.");
            }
            for key in keys {
                println!("{}", key);
            }
        }
        KeyAction::Add { path } => {
            keyring::Keyring::add(&path)?;
        }
    }
    Ok(())
}

pub async fn cmd_whatprovides(file: &str) -> Result<()> {
    let cache = PackageCache::load()?;
    let db    = InstalledDb::open()?;
    let mut found = false;
    for pkg in cache.all_packages() {
        if pkg.filename.as_deref().map_or(false, |f| f.contains(file)) {
            let status = if db.is_installed(&pkg.name) { "installed" } else { "available" };
            println!("{} ({})", pkg.name, status);
            found = true;
        }
    }
    if !found {
        println!("No package provides '{}'", file);
    }
    Ok(())
}

pub async fn cmd_provides(file: &str) -> Result<()> {
    let db    = InstalledDb::open()?;
    let mut found = false;
    for pkg in db.list_all()? {
        if pkg.files.contains(file) {
            println!("{}: {}", pkg.name, file);
            found = true;
        }
    }
    if !found {
        println!("No installed package provides '{}'", file);
    }
    Ok(())
}

pub async fn cmd_check_update() -> Result<()> {
    let cache  = PackageCache::load()?;
    let db     = InstalledDb::open()?;
    let solver = Solver::new(&cache, &db);
    let plan   = solver.resolve_upgrade()?;
    if plan.to_upgrade.is_empty() {
        println!("No updates available.");
    } else {
        println!("Available updates:");
        for pkg in &plan.to_upgrade {
            println!(
                "  {}-{} -> {}",
                pkg.name, plan.upgrade_from[&pkg.name], pkg.version
            );
        }
    }
    Ok(())
}

pub async fn cmd_import_dpkg() -> Result<()> {
    println!("Importing packages from dpkg/apt...");
    crate::import_dpkg::import_from_dpkg()?;
    Ok(())
}

pub fn print_version() {
    println!(
        "{} {} — Legendary Package Manager",
        "lpm".bold().bright_magenta(),
             env!("CARGO_PKG_VERSION").bold()
    );
    println!("Standalone Debian-compatible. No apt/dpkg required.");
    #[cfg(feature = "sat-solver")]
    println!("SAT solver: {}", "enabled".green().bold());
    #[cfg(not(feature = "sat-solver"))]
    println!("SAT solver: {}", "disabled (greedy resolver active)".yellow());
    println!("Log file: {}", crate::log::LOG_FILE.cyan());
}

pub fn print_help() {
    println!();
    println!(
        "{} {}",
        "lpm".bold().bright_magenta(),
             "— Legendary Package Manager".bold()
    );
    println!("  Standalone Debian-compatible. No apt/dpkg required.");
    println!();
    println!("{}", "Usage:".bold().yellow());
    println!("  {} {} [OPTIONS]", "lpm".bold(), "<command>".cyan());
    println!();
    println!("{}", "Package management:".bold().yellow());
    for (c, d) in &[
        ("install <pkg...>",  "Install packages and dependencies"),
        ("remove  <pkg...>",  "Remove packages"),
        ("upgrade",           "Upgrade all installed packages"),
        ("autoremove",        "Remove unneeded dependencies"),
    ] {
        println!("  {:<35} {}", c.cyan(), d.dimmed());
    }
    println!();
    println!("{}", "Repositories and cache:".bold().yellow());
    for (c, d) in &[
        ("update",                            "Refresh package metadata"),
        ("clean",                             "Remove cached package files"),
        ("repo list|add|remove|enable|disable","Manage repositories"),
        ("key  list|add",                     "Manage GPG keys"),
        ("import-dpkg",                       "Import existing packages from dpkg/apt"),
    ] {
        println!("  {:<42} {}", c.cyan(), d.dimmed());
    }
    println!();
    println!("{}", "Query:".bold().yellow());
    for (c, d) in &[
        ("search <query>",                "Search packages"),
        ("info   <package>",              "Show package details"),
        ("list [--installed|--upgrades]", "List packages"),
        ("history [undo|redo|diff|export]","Show transaction history"),
        ("whatprovides <file>",           "Find package providing a file"),
        ("provides <file>",               "Find installed package providing a file"),
        ("check-update",                  "Check for available upgrades"),
    ] {
        println!("  {:<42} {}", c.cyan(), d.dimmed());
    }
    println!();
    println!("{}", "Config:".bold().yellow());
    println!(
        "  {:<14} {}  {}",
        "Repos:".dimmed(),
             "/etc/lpm/sources-list.toml".cyan(),
             "(TOML, priorytet)".dimmed()
    );
    println!(
        "  {:<14} {}",
        "".dimmed(),
             "/etc/lpm/sources.list  lub  /etc/lpm/sources.list.d/*.list".cyan()
    );
    println!("  {:<14} {}", "DB:".dimmed(),    "/var/lib/lpm/lpm.db".cyan());
    println!("  {:<14} {}", "Cache:".dimmed(), "/var/cache/lpm/archives/".cyan());
    println!("  {:<14} {}", "Log:".dimmed(),   crate::log::LOG_FILE.cyan());
    println!();
}
