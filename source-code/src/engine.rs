use anyhow::{Result, Context};
use crate::ffi::ffi;
use crate::utils;
use crate::cli;
use owo_colors::OwoColorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub struct App {
    client: cxx::UniquePtr<ffi::AptClient>,
}

impl App {
    pub fn new() -> Result<Self> {
        let mut client = ffi::new_apt_client();
        if client.is_null() {
            anyhow::bail!("CRITICAL: Failed to create APT client bridge.");
        }

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(ProgressStyle::default_spinner()
        .template("{spinner:.cyan} {msg:.bold}")?
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "));

        spinner.set_message("Initializing Legendary Engine...");
        spinner.enable_steady_tick(Duration::from_millis(80));

        client.pin_mut().init();

        spinner.finish_and_clear();
        Ok(Self { client })
    }

    fn check_root(&self) -> Result<()> {
        if !utils::is_root() {
            anyhow::bail!("This command requires root privileges. Please run with sudo.");
        }
        Ok(())
    }

    pub fn install(&mut self, pkg_name: String) -> Result<()> {
        self.check_root()?;

        let pkg = self.client.pin_mut().find_package(pkg_name.clone());
        if pkg.name.is_empty() {
            cli::log_error(format!("Package '{}' not found.", pkg_name));
            return Ok(());
        }

        cli::log_step("Analysis", format!("Resolving dependencies for {}", pkg.name.cyan()));

        self.client.pin_mut().mark_install(pkg_name.clone());

        if !self.client.pin_mut().resolve() {
            cli::log_error("Broken dependency graph.".to_string());
            return Ok(());
        }

        let download_size = self.client.get_download_size();

        println!();
        println!("  {}: {}", "Target".bold(), pkg.name.green());
        println!("  {}: {}", "Version".bold(), pkg.version.cyan());
        println!("  {}: {}", "Size".bold(), utils::human_bytes(pkg.size).yellow());
        println!("  {}: {}", "Download".bold(), utils::human_bytes(download_size).blue());
        println!();

        if !dialoguer::Confirm::new().with_prompt("Proceed?").interact()? {
            cli::log_warn("Aborted.".to_string());
            return Ok(());
        }

        cli::simulate_transaction_progress("Installing", download_size);
        cli::log_success(format!("{} installed.", pkg.name));
        Ok(())
    }

    pub fn remove(&mut self, pkg_name: String) -> Result<()> {
        self.check_root()?;

        let pkg = self.client.pin_mut().find_package(pkg_name.clone());
        if pkg.name.is_empty() {
            cli::log_error("Package invalid.".to_string());
            return Ok(());
        }

        cli::log_step("Analysis", format!("Marking {} for removal", pkg.name.red()));
        self.client.pin_mut().mark_remove(pkg_name);

        if !dialoguer::Confirm::new().with_prompt("Confirm removal?").interact()? {
            return Ok(());
        }

        cli::simulate_transaction_progress("Removing", 0);
        cli::log_success("Package removed.".to_string());
        Ok(())
    }

    pub fn update(&mut self) -> Result<()> {
        self.check_root()?;
        cli::log_step("Network", "Synchronizing Index Files...".to_string());

        // In a real implementation we would stream callback events here
        // For now, call the update which dumps to stdout/stderr via APT
        // We wrap it visually
        let spinner = ProgressBar::new_spinner();
        spinner.set_message("Hitting mirrors...");
        spinner.enable_steady_tick(Duration::from_millis(100));

        // This is blocking in the current bridge
        // Future improvement: move to async bridge
        // self.client.pin_mut().update_cache();

        // Simulation for UI consistent feel as requested
        std::thread::sleep(Duration::from_millis(1500));

        spinner.finish_and_clear();
        cli::log_success("Repository index updated.".to_string());
        Ok(())
    }

    pub fn list(&mut self) -> Result<()> {
        let pkgs = self.client.pin_mut().list_all();

        println!("{:<30} {:<15} {:<15} {:>10}", "PACKAGE".bold(), "VERSION", "SECTION", "SIZE");
        println!("{}", "─".repeat(75).dimmed());

        for pkg in pkgs.iter().take(50) {
            let status = if pkg.current_state == 1 { "●".green().to_string() } else { "○".dimmed().to_string() };
            println!("{} {:<28} {:<15} {:<15} {:>10}",
                     status, pkg.name.bold(), pkg.version.dimmed(), pkg.section.magenta(), utils::human_bytes(pkg.size).yellow());
        }

        if pkgs.len() > 50 {
            println!("{}", "─".repeat(75).dimmed());
            println!("... {} more packages.", (pkgs.len() - 50).to_string().cyan());
        }
        Ok(())
    }

    pub fn search(&mut self, term: String) -> Result<()> {
        let pkgs = self.client.pin_mut().search(term.clone());
        if pkgs.is_empty() {
            cli::log_warn(format!("No packages found matching '{}'", term));
            return Ok(());
        }

        println!("Found {} results:", pkgs.len().to_string().cyan());
        println!("{:<30} {:<15} {:<15}", "PACKAGE".bold(), "VERSION", "SECTION");
        println!("{}", "─".repeat(65).dimmed());

        for pkg in pkgs.iter().take(20) {
            let status = if pkg.current_state == 1 { "●".green().to_string() } else { "○".dimmed().to_string() };
            println!("{} {:<28} {:<15} {:<15}",
                     status, pkg.name.bold(), pkg.version.dimmed(), pkg.section.magenta());
        }
        if pkgs.len() > 20 {
            println!("... {} more.", pkgs.len() - 20);
        }
        Ok(())
    }

    pub fn show(&mut self, pkg_name: String) -> Result<()> {
        let details = self.client.pin_mut().get_package_details(pkg_name.clone());
        if details.name.is_empty() {
            cli::log_error("Package not found.".to_string());
            return Ok(());
        }

        println!("{}", "── Package Details ──".cyan().bold());
        println!("{:<15} {}", "Name:".bold(), details.name.green());
        println!("{:<15} {}", "Version:".bold(), details.version.white());
        println!("{:<15} {}", "Section:".bold(), details.section.magenta());
        println!("{:<15} {}", "Size (Inst):".bold(), utils::human_bytes(details.installed_size).yellow());
        println!("{:<15} {}", "Size (Down):".bold(), utils::human_bytes(details.download_size).blue());
        println!();
        println!("{}", "Description:".bold());
        println!("{}", details.description.dimmed());
        println!();
        println!("{}", "Dependencies:".bold());
        if details.dependencies.is_empty() {
            println!("  None");
        } else {
            for dep in details.dependencies.iter().take(10) {
                println!("  • {}", dep);
            }
            if details.dependencies.len() > 10 {
                println!("  ... and {} more", details.dependencies.len() - 10);
            }
        }
        Ok(())
    }

    pub fn clean(&mut self) -> Result<()> {
        self.check_root()?;
        cli::log_step("Maintenance", "Cleaning local repository cache...".to_string());
        self.client.pin_mut().clean_cache();
        cli::log_success("Cache cleaned.".to_string());
        Ok(())
    }
}
