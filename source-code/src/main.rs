mod repo;
mod solver;
mod installer;
mod utils;
mod db;
mod ffi;
mod plugins;
mod logger;

use anyhow::Result;
use console::style;
use crate::plugins::PluginManager;
use crate::db::Database;
use std::sync::Arc;
use std::path::Path;
use comfy_table::{Table, presets, Attribute, Cell, Color, ContentArrangement};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize Logger
    if let Err(e) = logger::init() {
        eprintln!("Warning: Failed to initialize logger: {}", e);
    }

    let mut parser = lexopt::Parser::from_env();
    let mut cmd = None;
    let mut args: Vec<String> = Vec::new();

    while let Some(arg) = parser.next()? {
        use lexopt::prelude::*;
        match arg {
            Value(v) if cmd.is_none() => cmd = Some(v.string()?),
            Value(v) => args.push(v.string()?),
            _ => (),
        }
    }

    let command = match cmd {
        Some(c) => c,
        None => {
            print_help();
            return Ok(());
        }
    };

    logger::info(&format!("Command started: {} {:?}", command, args));

    let plugin_manager = Arc::new(PluginManager::new()?);
    let _ = plugin_manager.load_all();

    match command.as_str() {
        "refresh" | "update" => {
            if !utils::is_root() {
                eprintln!("Error: Root privileges required for update.");
                logger::error("Update failed: Root privileges required");
                std::process::exit(1);
            }
            println!("Updating repositories...");
            match repo::refresh_metadata().await {
                Ok(_) => {
                    println!("Metadata updated successfully.");
                    logger::info("Repositories updated successfully");
                }
                Err(e) => {
                    eprintln!("Error updating repositories: {}", e);
                    logger::error(&format!("Update failed: {}", e));
                }
            }
        }
        "search" => {
            if args.is_empty() {
                eprintln!("Usage: search <query>");
                return Ok(());
            }
            let results = repo::search(&args[0]).await?;
            if results.is_empty() {
                println!("No packages found.");
                check_cache();
            } else {
                let mut table = Table::new();
                table.load_preset(presets::UTF8_FULL)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_header(vec![
                    Cell::new("Name").add_attribute(Attribute::Bold),
                            Cell::new("Version").add_attribute(Attribute::Bold),
                            Cell::new("Description")
                ]);

                for pkg in results {
                    let desc = pkg.description.lines().next().unwrap_or("");
                    table.add_row(vec![
                        Cell::new(&pkg.name).fg(Color::Green),
                                  Cell::new(&pkg.version),
                                  Cell::new(desc)
                    ]);
                }
                println!("{}", table);
            }
        }
        "install" => {
            if let Err(e) = handle_transaction(&args, false, plugin_manager).await {
                logger::error(&format!("Install failed: {}", e));
                eprintln!("Error: {}", e);
            }
        }
        "remove" => {
            if let Err(e) = handle_transaction(&args, true, plugin_manager).await {
                logger::error(&format!("Remove failed: {}", e));
                eprintln!("Error: {}", e);
            }
        }
        "history" => {
            let db = Database::load();
            let history = db.get_history()?;
            if history.is_empty() {
                println!("No history available.");
            } else {
                let mut table = Table::new();
                table.load_preset(presets::UTF8_HORIZONTAL_ONLY)
                .set_header(vec!["Date", "Action", "Command", "Packages"]);

                for h in history {
                    table.add_row(vec![
                        h.timestamp.format("%Y-%m-%d %H:%M").to_string(),
                                  h.action,
                                  h.command,
                                  h.packages.join(", ")
                    ]);
                }
                println!("{}", table);
            }
        }
        "autoremove" => {
            if !utils::is_root() {
                eprintln!("Error: Root privileges required.");
                return Ok(());
            }
            let db = Database::load();
            let orphans = db.get_orphans();

            if orphans.is_empty() {
                println!("No orphan packages to remove.");
                return Ok(());
            }

            println!("The following packages are no longer needed and will be removed:");
            println!("{}", orphans.join(" "));

            println!("Continue? [Y/n]");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if input.trim().to_lowercase() == "y" {
                // Removed unused 'mut'
                let args_rem: Vec<String> = orphans.iter().map(|s| format!("{}-", s)).collect();
                logger::info(&format!("Autoremoving: {:?}", orphans));
                let tx = solver::resolve(&args_rem).await?;
                installer::execute(tx, plugin_manager, &[]).await?;
            }
        }
        "verify" => {
            installer::verify_installation()?;
        }
        "clean" => {
            if !utils::is_root() {
                eprintln!("Error: Root privileges required.");
                return Ok(());
            }
            println!("Cleaning package cache...");
            let cache_dir = Path::new("/var/cache/lpm/archives");
            let lists_dir = Path::new("/var/lib/lpm/lists");

            let mut freed = 0;
            if cache_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(cache_dir) {
                    for e in entries {
                        if let Ok(e) = e {
                            if let Ok(meta) = e.metadata() { freed += meta.len(); }
                            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                                let _ = std::fs::remove_file(e.path());
                            }
                        }
                    }
                }
            }
            if lists_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(lists_dir) {
                    for e in entries {
                        if let Ok(e) = e {
                            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                                let _ = std::fs::remove_file(e.path());
                            }
                        }
                    }
                }
            }
            let msg = format!("Cleaned {} of data.", utils::human_bytes(freed as i64));
            println!("{}", msg);
            logger::info(&msg);
        }
        _ => {
            eprintln!("Unknown command: {}", command);
            print_help();
        }
    }

    Ok(())
}

async fn handle_transaction(args: &[String], is_remove: bool, plugins: Arc<PluginManager>) -> Result<()> {
    if args.is_empty() {
        println!("No packages specified.");
        return Ok(());
    }
    if !utils::is_root() {
        eprintln!("Error: Root privileges required.");
        return Ok(());
    }

    // Prepare arguments for solver (append '-' for removal)
    let solve_args: Vec<String> = if is_remove {
        args.iter().map(|a| if a.ends_with("-") { a.clone() } else { format!("{}-", a) }).collect()
    } else {
        args.to_vec()
    };

    println!("Solving dependencies...");
    let tx = solver::resolve(&solve_args).await?;

    if tx.install.is_empty() && tx.remove.is_empty() && tx.upgrade.is_empty() {
        println!("Nothing to do.");
        return Ok(());
    }

    // Print Transaction Table
    let mut table = Table::new();
    table.load_preset(presets::NOTHING)
    .set_header(vec!["Action", "Package", "Version", "Size"]);

    for p in &tx.install {
        table.add_row(vec![
            Cell::new("Install").fg(Color::Green),
                      Cell::new(&p.name).add_attribute(Attribute::Bold),
                      Cell::new(&p.version),
                      Cell::new(utils::human_bytes(p.size as i64))
        ]);
    }
    for p in &tx.upgrade {
        table.add_row(vec![
            Cell::new("Upgrade").fg(Color::Yellow),
                      Cell::new(&p.name).add_attribute(Attribute::Bold),
                      Cell::new(&p.version),
                      Cell::new(utils::human_bytes(p.size as i64))
        ]);
    }
    for p in &tx.remove {
        table.add_row(vec![
            Cell::new("Remove").fg(Color::Red),
                      Cell::new(&p.name).add_attribute(Attribute::Bold),
                      Cell::new(&p.version),
                      Cell::new("-")
        ]);
    }

    println!("{}", table);
    if tx.total_download > 0 {
        println!("\nTotal Download: {}", style(utils::human_bytes(tx.total_download as i64)).bold());
    }

    println!("Continue? [Y/n]");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" && !input.trim().is_empty() {
        println!("Aborted.");
        logger::info("Transaction aborted by user");
        return Ok(());
    }

    logger::info(&format!("Executing transaction: Install={:?}, Remove={:?}",
                          tx.install.iter().map(|p| &p.name).collect::<Vec<_>>(),
                          tx.remove.iter().map(|p| &p.name).collect::<Vec<_>>()
    ));

    installer::execute(tx, plugins, args).await?;
    Ok(())
}

fn check_cache() {
    let cache_path = Path::new("/var/lib/lpm/lists");
    let is_empty = !cache_path.exists() || cache_path.read_dir().map(|mut i| i.next().is_none()).unwrap_or(true);
    if is_empty {
        println!("\nTip: Cache is empty. Run 'sudo lpm update'.");
    }
}

fn print_help() {
    println!("Legendary Package Manager v0.6");
    println!("Commands:");
    println!("  update      Update repositories");
    println!("  install     Install packages");
    println!("  remove      Remove packages");
    println!("  clean       Clear local cache");
    println!("  search      Search for packages");
    println!("  history     Show transaction history");
    println!("  autoremove  Remove orphan packages");
    println!("  verify      Verify installed files integrity");
}
