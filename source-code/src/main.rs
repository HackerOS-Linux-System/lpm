mod ffi;
mod utils;
mod cli;
mod engine;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

fn main() -> Result<()> {
    // Parse arguments
    use lexopt::prelude::*;
    let mut parser = lexopt::Parser::from_env();
    let mut cmd = None;
    let mut arg = None;

    while let Some(arg_val) = parser.next()? {
        match arg_val {
            Value(v) if cmd.is_none() => cmd = Some(v.string()?),
            Value(v) if arg.is_none() => arg = Some(v.string()?),
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

    // Show banner only for interactive commands, not plumbing
    if command != "help" && command != "version" {
        cli::print_header();
    }

    let mut app = engine::App::new().context("Engine initialization failed")?;

    match command.as_str() {
        "install" => {
            if let Some(pkg) = arg {
                app.install(pkg)?;
            } else {
                cli::log_error("Usage: legend install <package>".to_string());
            }
        }
        "remove" => {
            if let Some(pkg) = arg {
                app.remove(pkg)?;
            } else {
                cli::log_error("Usage: legend remove <package>".to_string());
            }
        }
        "list" => {
            app.list()?;
        }
        "update" => {
            app.update()?;
        }
        "search" => {
            if let Some(term) = arg {
                app.search(term)?;
            } else {
                cli::log_error("Usage: legend search <term>".to_string());
            }
        }
        "show" => {
            if let Some(pkg) = arg {
                app.show(pkg)?;
            } else {
                cli::log_error("Usage: legend show <package>".to_string());
            }
        }
        "clean" => {
            app.clean()?;
        }
        "help" => {
            cli::print_help();
        }
        _ => {
            cli::log_error(format!("Unknown command '{}'", command));
        }
    }

    Ok(())
}
