/// DNF-inspired terminal UI for lpm.
///
/// Visual language:
///   ● (cyan)   = informational header / section title
///   ✓ (green)  = success
///   ✗ (red)    = error / fatal
///   ! (yellow) = warning
///   → (blue)   = action in progress

use bytesize::ByteSize;
use owo_colors::OwoColorize;
use std::io::{self, Write};

use crate::db::HistoryEntry;
use crate::solver::TransactionPlan;

// ─────────────────────────────────────────────────────────────
//  Width helper
// ─────────────────────────────────────────────────────────────

pub fn term_width() -> usize {
    terminal_size::terminal_size()
    .map(|(w, _)| w.0 as usize)
    .unwrap_or(80)
}

fn sep(ch: char) -> String {
    ch.to_string().repeat(term_width().min(80))
}

// ─────────────────────────────────────────────────────────────
//  Headers
// ─────────────────────────────────────────────────────────────

pub fn header(title: &str) {
    println!();
    println!("{} {}", "●".cyan().bold(), title.bold());
}

pub fn section(title: &str) {
    println!("{}", sep('─').dimmed());
    println!(" {}", title.bold().underline());
    println!("{}", sep('─').dimmed());
}

// ─────────────────────────────────────────────────────────────
//  Status messages
// ─────────────────────────────────────────────────────────────

pub fn ok(msg: &str) {
    println!("  {} {}", "✓".green().bold(), msg);
}

pub fn warn(msg: &str) {
    eprintln!("  {} {}", "!".yellow().bold(), msg.yellow());
}

pub fn fatal(msg: &str) {
    eprintln!();
    eprintln!("  {} {}", "✗".red().bold(), msg.bold().red());
    eprintln!();
}

pub fn info(msg: &str) {
    println!("  {} {}", "→".blue().bold(), msg);
}

// ─────────────────────────────────────────────────────────────
//  Transaction Summary (DNF-style)
// ─────────────────────────────────────────────────────────────

pub fn print_transaction_summary(plan: &TransactionPlan) {
    let w = term_width().min(80);
    let bar = "=".repeat(w);

    println!();
    println!("{}", bar.blue().bold());
    println!(" {}", "Transaction Summary".bold());
    println!("{}", bar.blue().bold());

    if !plan.to_install.is_empty() {
        println!(
            " {:<12} {:>5} Package{}",
            "Install:".green().bold(),
                 plan.to_install.len(),
                 if plan.to_install.len() == 1 { "" } else { "s" }
        );
        print_pkg_namelist(&plan.to_install.iter().map(|p| (&p.name, &p.version)).collect::<Vec<_>>(), "green");
    }

    if !plan.to_upgrade.is_empty() {
        println!(
            " {:<12} {:>5} Package{}",
            "Upgrade:".yellow().bold(),
                 plan.to_upgrade.len(),
                 if plan.to_upgrade.len() == 1 { "" } else { "s" }
        );
        // Show old→new
        for pkg in &plan.to_upgrade {
            let old = plan.upgrade_from.get(&pkg.name).map(|s| s.as_str()).unwrap_or("?");
            println!(
                "   {} {}-{} {} {}",
                "↑".yellow().bold(),
                     pkg.name.bold(),
                     old.dimmed(),
                     "→".dimmed(),
                     pkg.version.green().bold()
            );
        }
    }

    if !plan.to_remove.is_empty() {
        println!(
            " {:<12} {:>5} Package{}",
            "Remove:".red().bold(),
                 plan.to_remove.len(),
                 if plan.to_remove.len() == 1 { "" } else { "s" }
        );
        let names: Vec<String> = plan.to_remove.clone();
        let pairs: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "")).collect();
        print_name_list(&pairs, "red");
    }

    if !plan.to_autoremove.is_empty() {
        println!(
            " {:<12} {:>5} Package{}",
            "Autoremove:".red().bold(),
                 plan.to_autoremove.len(),
                 if plan.to_autoremove.len() == 1 { "" } else { "s" }
        );
    }

    println!();
    println!("{}", bar.blue().bold());

    let total = plan.to_install.len() + plan.to_upgrade.len();
    println!(" {:<24} {}", "Total packages:".bold(), total);

    if plan.download_bytes > 0 {
        println!(
            " {:<24} {}",
            "Total download size:".bold(),
                 ByteSize(plan.download_bytes).to_string().cyan()
        );
    }

    if plan.install_bytes > 0 {
        println!(
            " {:<24} {}",
            "Installed size:".bold(),
                 ByteSize(plan.install_bytes).to_string().cyan()
        );
    }

    if plan.freed_bytes > 0 {
        println!(
            " {:<24} {}",
            "Freed space:".bold(),
                 ByteSize(plan.freed_bytes).to_string().yellow()
        );
    }

    if !plan.warnings.is_empty() {
        println!();
        for w in &plan.warnings {
            warn(w);
        }
    }

    println!("{}", bar.blue().bold());
    println!();
}

// ─────────────────────────────────────────────────────────────
//  Package name lists (wrapped)
// ─────────────────────────────────────────────────────────────

fn print_pkg_namelist(pairs: &[(&String, &String)], color: &str) {
    let w = term_width().min(80);
    let indent = "   ";
    let mut line = indent.to_string();
    let mut line_len = indent.len();

    for (name, ver) in pairs {
        let entry   = format!("{}-{} ", name, ver);
        let colored = match color {
            "green"  => format!("{}-{} ", name.green().bold(), ver.dimmed()),
            "yellow" => format!("{}-{} ", name.yellow().bold(), ver.dimmed()),
            "red"    => format!("{}-{} ", name.red().bold(), ver.dimmed()),
            _        => entry.clone(),
        };

        if line_len + entry.len() > w && line_len > indent.len() {
            println!("{}", line);
            line     = format!("{}{}", indent, colored);
            line_len = indent.len() + entry.len();
        } else {
            line.push_str(&colored);
            line_len += entry.len();
        }
    }

    if line_len > indent.len() { println!("{}", line); }
}

fn print_name_list(pairs: &[(&str, &str)], color: &str) {
    let w = term_width().min(80);
    let indent = "   ";
    let mut line = indent.to_string();
    let mut line_len = indent.len();

    for (name, _) in pairs {
        let entry   = format!("{} ", name);
        let colored = match color {
            "red"    => format!("{} ", name.red().bold()),
            "yellow" => format!("{} ", name.yellow().bold()),
            _        => format!("{} ", name.green().bold()),
        };

        if line_len + entry.len() > w && line_len > indent.len() {
            println!("{}", line);
            line     = format!("{}{}", indent, colored);
            line_len = indent.len() + entry.len();
        } else {
            line.push_str(&colored);
            line_len += entry.len();
        }
    }

    if line_len > indent.len() { println!("{}", line); }
}

// ─────────────────────────────────────────────────────────────
//  Confirm prompt
// ─────────────────────────────────────────────────────────────

pub fn confirm(prompt: &str) -> io::Result<bool> {
    print!("{} [{}{}]: ",
           prompt.bold(),
           "y".green().bold(),
           "/N".dimmed()
    );
    io::stdout().flush()?;

    let mut s = String::new();
    io::stdin().read_line(&mut s)?;

    let t = s.trim().to_lowercase();
    Ok(t == "y" || t == "yes")
}

// ─────────────────────────────────────────────────────────────
//  Search results
// ─────────────────────────────────────────────────────────────

pub fn print_search_results(
    results:   &[&crate::package::Package],
    installed: &crate::db::InstalledDb,
) {
    for pkg in results {
        let inst_tag = if installed.is_installed(&pkg.name) {
            format!(" @{}", "installed".green().bold())
        } else {
            String::new()
        };

        println!(
            "{}/{} {}{}",
            pkg.name.bold().bright_white(),
                 pkg.version.bright_cyan(),
                 pkg.architecture.dimmed(),
                 inst_tag
        );

        if let Some(ref d) = pkg.description_short {
            println!("  {}", d.dimmed());
        }
    }
}

// ─────────────────────────────────────────────────────────────
//  Package info
// ─────────────────────────────────────────────────────────────

pub fn print_package_info(
    pkg:       &crate::package::Package,
    installed: &crate::db::InstalledDb,
) {
    let w = term_width().min(80);
    let sep_line = "─".repeat(w);

    println!("{}", sep_line.dimmed());

    let field = |label: &str, value: &str| {
        println!("{:<22}: {}", label.bold(), value);
    };

    field("Name",         &pkg.name);
    field("Version",      &pkg.version);
    field("Architecture", &pkg.architecture);

    let status = if installed.is_installed(&pkg.name) {
        "Installed".green().bold().to_string()
    } else {
        "Available".yellow().to_string()
    };
    field("Status",  &status);

    if let Some(ref s) = pkg.section    { field("Section",     s); }
    if let Some(ref m) = pkg.maintainer { field("Maintainer",  m); }

    if let Some(sz) = pkg.installed_size_kb {
        field("Installed Size", &format!("{}", ByteSize(sz * 1024)));
    }
    if let Some(sz) = pkg.download_size {
        field("Download Size",  &format!("{}", ByteSize(sz)));
    }
    if let Some(ref h) = pkg.homepage   { field("Homepage",    h); }
    if let Some(ref d) = pkg.depends    { field("Depends",     d); }
    if let Some(ref r) = pkg.recommends { field("Recommends",  r); }
    if let Some(ref s) = pkg.suggests   { field("Suggests",    s); }
    if let Some(ref c) = pkg.conflicts  { field("Conflicts",   c); }

    println!("{}", sep_line.dimmed());

    if let Some(ref d) = pkg.description_short {
        println!("{}", d.bold());
    }
    if let Some(ref d) = pkg.description_long {
        for line in d.lines() {
            let line = line.trim();
            if line == "." {
                println!();
            } else {
                println!("  {}", line.dimmed());
            }
        }
    }

    println!("{}", sep_line.dimmed());
}

// ─────────────────────────────────────────────────────────────
//  List
// ─────────────────────────────────────────────────────────────

pub fn print_list_entry(
    name:       &str,
    version:    &str,
    arch:       &str,
    installed:  bool,
    upgradeable: bool,
    new_version: Option<&str>,
) {
    let mut tags = Vec::new();
    if installed  { tags.push("installed".green().bold().to_string()); }
    if upgradeable {
        if let Some(nv) = new_version {
            tags.push(format!("upgradeable to: {}", nv.bright_yellow().bold()));
        } else {
            tags.push("upgradeable".yellow().bold().to_string());
        }
    }

    let tag_str = if tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", tags.join(", "))
    };

    println!(
        "{}/{} {}{}{}",
        name.bold(),
             version.bright_cyan(),
             arch.dimmed(),
             tag_str,
             ""
    );
}

// ─────────────────────────────────────────────────────────────
//  History
// ─────────────────────────────────────────────────────────────

pub fn print_history(entries: &[HistoryEntry]) {
    let w = term_width().min(80);
    println!("{}", "─".repeat(w).dimmed());
    println!(
        " {:<4}  {:<12}  {:<30}  {:<16}  {}",
        "ID".bold(),
             "Action".bold(),
             "Package".bold(),
             "Version".bold(),
             "Date".bold()
    );
    println!("{}", "─".repeat(w).dimmed());

    for e in entries {
        let action_str = match e.action.as_str() {
            "install" => e.action.green().bold().to_string(),
            "remove"  => e.action.red().bold().to_string(),
            "upgrade" => e.action.yellow().bold().to_string(),
            other     => other.to_string(),
        };

        let ver_str = match (&e.old_ver, &e.new_ver) {
            (None,    Some(nv)) => nv.clone(),
            (Some(ov), None)   => ov.clone(),
            (Some(ov), Some(nv)) => format!("{} → {}", ov.dimmed(), nv.bright_cyan()),
            _ => String::new(),
        };

        let date_str = e.timestamp.format("%Y-%m-%d %H:%M").to_string();

        println!(
            " {:<4}  {:<20}  {:<30}  {:<24}  {}",
            e.id.to_string().dimmed(),
                 action_str,
                 e.package.bold(),
                 ver_str,
                 date_str.dimmed()
        );
    }

    println!("{}", "─".repeat(w).dimmed());
}
