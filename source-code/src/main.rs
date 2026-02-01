mod ffi;
mod utils;
mod cli;
mod engine;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    // Parse arguments
    use lexopt::prelude::*;
    let mut parser = lexopt::Parser::from_env();
    let mut cmd = None;
    let mut args: Vec<String> = Vec::new();

    while let Some(arg_val) = parser.next()? {
        match arg_val {
            Value(v) if cmd.is_none() => cmd = Some(v.string()?),
            Value(v) => args.push(v.string()?),
            Long("help") | Short('h') => {
                cli::print_help();
                return Ok(());
            }
            Long("version") | Short('v') => {
                println!("Legendary v0.2.0");
                return Ok(());
            }
            _ => return Err(anyhow::anyhow!("Unknown argument.")),
        }
    }

    if cmd.is_none() {
        cli::print_help();
        return Ok(());
    }

    let command = cmd.unwrap();

    // Show banner only for interactive commands
    if command != "help" && command != "version" {
        cli::print_header();
    }

    // Determine lock requirement
    let needs_lock = match command.as_str() {
        "install" | "remove" | "update" | "clean" | "upgrade" | "full-upgrade" | "autoremove" | "add-repo" => true,
        _ => false,
    };

    let mut app = engine::App::new(needs_lock).context("Engine initialization failed")?;

    match command.as_str() {
        "install" => {
            if args.is_empty() {
                cli::log_error("Usage: legend install <package1> [package2] ...".to_string());
            } else {
                app.install(args)?;
            }
        }
        "remove" => {
            if args.is_empty() {
                cli::log_error("Usage: legend remove <package1> ...".to_string());
            } else {
                app.remove(args)?;
            }
        }
        "update" => app.update()?,
        "upgrade" => app.upgrade(false)?,
        "full-upgrade" => app.upgrade(true)?,
        "autoremove" => app.autoremove()?,
        "list" => app.list()?,
        "search" => {
            if args.is_empty() {
                cli::log_error("Usage: legend search <term>".to_string());
            } else {
                app.search(args[0].clone())?;
            }
        }
        "show" => {
            if args.is_empty() {
                cli::log_error("Usage: legend show <package>".to_string());
            } else {
                app.show(args[0].clone())?;
            }
        }
        "clean" => app.clean()?,
        "list-repos" => app.list_repos()?,
        "add-repo" => {
            if args.is_empty() {
                cli::log_error("Usage: legend add-repo \"deb http://...\"".to_string());
            } else {
                app.add_repo(args.join(" "))?;
            }
        }
        "help" => cli::print_help(),
        _ => cli::log_error(format!("Unknown command '{}'", command)),
    }

    Ok(())
}
