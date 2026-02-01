use anyhow::Result;
use crate::ffi::{self, ffi as cxx_ffi};
use crate::utils;
use crate::cli;
use owo_colors::OwoColorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use std::fs::{self, OpenOptions};
use std::io::Write;

pub struct App {
    client: cxx::UniquePtr<cxx_ffi::AptClient>,
}

impl App {
    pub fn new(needs_lock: bool) -> Result<Self> {
        let mut client = cxx_ffi::new_apt_client();
        if client.is_null() {
            anyhow::bail!("CRITICAL: Failed to create APT client bridge.");
        }

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(ProgressStyle::default_spinner()
        .template("{spinner:.cyan} {msg:.bold}")?
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "));

        spinner.set_message("Initializing Legendary Engine...");
        spinner.enable_steady_tick(Duration::from_millis(80));

        client.pin_mut().init(needs_lock);

        spinner.finish_and_clear();
        Ok(Self { client })
    }

    fn check_root(&self) -> Result<()> {
        if !utils::is_root() {
            anyhow::bail!("Access Denied: You are not root. Please use sudo.");
        }
        Ok(())
    }

    fn handle_commit_error(&mut self) {
        let err = self.client.pin_mut().get_last_error();
        println!();
        if !err.is_empty() {
            cli::log_error(format!("Transaction failed:\n{}", err.red()));
        } else {
            cli::log_error("Transaction failed with unknown error. (Check console output above)".to_string());
        }
    }

    fn print_summary(&mut self) -> Result<bool> {
        let summary = self.client.pin_mut().get_transaction_changes();

        let mut anything_to_do = false;

        if !summary.to_install.is_empty() {
            anything_to_do = true;
            println!("{}", "Packages to be INSTALLED:".green().bold());
            for p in summary.to_install {
                println!("  + {}", p);
            }
            println!();
        }

        if !summary.to_upgrade.is_empty() {
            anything_to_do = true;
            println!("{}", "Packages to be UPGRADED:".yellow().bold());
            for p in summary.to_upgrade {
                println!("  ↑ {}", p);
            }
            println!();
        }

        if !summary.to_remove.is_empty() {
            anything_to_do = true;
            println!("{}", "Packages to be REMOVED:".red().bold());
            for p in summary.to_remove {
                println!("  - {}", p);
            }
            println!();
        }

        if !anything_to_do {
            cli::log_success("Nothing to do.".to_string());
            return Ok(false);
        }

        let download_size = self.client.get_download_size();
        println!("{} {}", "Download Size:".bold(), utils::human_bytes(download_size).blue());
        println!();

        Ok(true)
    }

    fn execute_transaction(&mut self) -> Result<()> {
        if !dialoguer::Confirm::new().with_prompt("Confirm transaction?").interact()? {
            cli::log_warn("Aborted by user.".to_string());
            return Ok(());
        }

        cli::log_step("Execution", "Processing transaction...".to_string());
        ffi::init_progress_bar(100, "Preparing...");
        let success = self.client.pin_mut().commit_changes();
        ffi::clear_progress_bar();

        if !success {
            self.handle_commit_error();
        } else {
            cli::log_success("Transaction completed successfully.".to_string());
        }
        Ok(())
    }

    pub fn install(&mut self, pkgs: Vec<String>) -> Result<()> {
        self.check_root()?;

        for pkg_name in pkgs {
            let pkg = self.client.pin_mut().find_package(pkg_name.clone());
            if pkg.name.is_empty() {
                cli::log_error(format!("Package '{}' not found.", pkg_name));
                continue;
            }
            cli::log_step("Analysis", format!("Marking {} for installation", pkg.name.cyan()));
            self.client.pin_mut().mark_install(pkg_name);
        }

        if !self.client.pin_mut().resolve() {
            cli::log_error("Dependency resolution failed.".to_string());
            return Ok(());
        }

        if self.print_summary()? {
            self.execute_transaction()?;
        }
        Ok(())
    }

    pub fn remove(&mut self, pkgs: Vec<String>) -> Result<()> {
        self.check_root()?;

        for pkg_name in pkgs {
            cli::log_step("Analysis", format!("Marking {} for removal", pkg_name.red()));
            self.client.pin_mut().mark_remove(pkg_name);
        }

        if !self.client.pin_mut().resolve() {
            cli::log_error("Dependency resolution failed.".to_string());
            return Ok(());
        }

        if self.print_summary()? {
            self.execute_transaction()?;
        }
        Ok(())
    }

    pub fn update(&mut self) -> Result<()> {
        self.check_root()?;
        cli::log_step("Network", "Synchronizing Package Index...".to_string());
        ffi::init_progress_bar(100, "Updating sources...");
        self.client.pin_mut().update_cache();
        ffi::clear_progress_bar();

        let err = self.client.pin_mut().get_last_error();
        if !err.is_empty() {
            cli::log_warn(format!("Update completed with warnings:\n{}", err));
        } else {
            cli::log_success("All repositories are up to date.".to_string());
        }
        Ok(())
    }

    pub fn upgrade(&mut self, full: bool) -> Result<()> {
        self.check_root()?;
        cli::log_step("Analysis", format!("Calculating {}...", if full { "Full Upgrade" } else { "Upgrade" }));

        if full {
            self.client.pin_mut().mark_full_upgrade();
        } else {
            self.client.pin_mut().mark_upgrade();
        }

        if self.print_summary()? {
            self.execute_transaction()?;
        }
        Ok(())
    }

    pub fn autoremove(&mut self) -> Result<()> {
        self.check_root()?;
        cli::log_step("Analysis", "Finding unused dependencies...".to_string());
        self.client.pin_mut().mark_autoremove();

        if self.print_summary()? {
            self.execute_transaction()?;
        }
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
        Ok(())
    }

    pub fn search(&mut self, term: String) -> Result<()> {
        let mut pkgs = self.client.pin_mut().search(term.clone());
        if pkgs.is_empty() {
            cli::log_warn(format!("No packages found matching '{}'", term));
            return Ok(());
        }

        // Smart Sort:
        // 1. Exact Name match (High score)
        // 2. Name starts with term
        // 3. Name contains term
        // 4. Description match (implicit via C++ search)

        let t_lower = term.to_lowercase();
        pkgs.sort_by(|a, b| {
            let score_a = get_score(&a.name, &t_lower);
            let score_b = get_score(&b.name, &t_lower);
            score_b.cmp(&score_a) // Descending
        });

        println!("Found {} results:", pkgs.len().to_string().cyan());
        println!("{:<30} {:<15} {:<15} {:>10}", "PACKAGE".bold(), "VERSION", "SECTION", "SIZE");
        println!("{}", "─".repeat(75).dimmed());

        for pkg in pkgs.iter().take(20) {
            let status = if pkg.current_state == 1 { "●".green().to_string() } else { "○".dimmed().to_string() };
            println!("{} {:<28} {:<15} {:<15} {:>10}",
                     status, pkg.name.bold(), pkg.version.dimmed(), pkg.section.magenta(), utils::human_bytes(pkg.size).yellow());
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
        println!();
        println!("{}", "Description:".bold());
        println!("{}", details.description.dimmed());
        Ok(())
    }

    pub fn clean(&mut self) -> Result<()> {
        self.check_root()?;
        cli::log_step("Maintenance", "Cleaning local repository cache...".to_string());
        self.client.pin_mut().clean_cache();
        cli::log_success("Cache cleaned.".to_string());
        Ok(())
    }

    // --- Repo Management ---

    pub fn list_repos(&self) -> Result<()> {
        println!("{}", "── Active Repositories ──".cyan().bold());

        let paths = vec!["/etc/apt/sources.list"];
        // Also walk sources.list.d if possible, but keeping simple for now

        for p in paths {
            if let Ok(content) = fs::read_to_string(p) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.starts_with('#') && !trimmed.is_empty() {
                        println!(" {}", trimmed);
                    }
                }
            }
        }
        // Scan directory
        if let Ok(entries) = fs::read_dir("/etc/apt/sources.list.d/") {
            for entry in entries.flatten() {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.starts_with('#') && !trimmed.is_empty() {
                            println!(" {}", trimmed);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn add_repo(&self, line: String) -> Result<()> {
        self.check_root()?;
        let path = "/etc/apt/sources.list.d/legendary-added.list";
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{}", line)?;
        cli::log_success(format!("Added repository to {}", path));
        Ok(())
    }
}

fn get_score(name: &str, term: &str) -> i32 {
    let n_lower = name.to_lowercase();
    if n_lower == term { return 100; }
    if n_lower.starts_with(term) { return 80; }
    if n_lower.contains(term) { return 60; }
    0
}
