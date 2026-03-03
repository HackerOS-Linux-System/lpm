use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use std::io::{self, Write};
use std::time::Duration;

use crate::db::HistoryEntry;
use crate::package::Package;
use crate::solver::TransactionPlan;

// ─────────────────────────────────────────────────────────────
//  Terminal width
// ─────────────────────────────────────────────────────────────

pub fn term_width() -> usize {
    terminal_size::terminal_size()
    .map(|(w, _)| w.0 as usize)
    .unwrap_or(80)
    .min(120)
}

// ─────────────────────────────────────────────────────────────
//  Simple status lines
// ─────────────────────────────────────────────────────────────

pub fn last_metadata_check() {
    let now = chrono::Local::now();
    println!(
        "Last metadata expiration check: {} on {}.",
        "0:00:00 ago".dimmed(),
             now.format("%a %d %b %Y %H:%M:%S").to_string().dimmed()
    );
}

pub fn deps_resolved() {
    println!("{}", "Dependencies resolved.".bold());
}

pub fn nothing_to_do() {
    println!("{}", "Nothing to do.".bold());
}

pub fn complete() {
    println!("{}", "Complete!".green().bold());
}

pub fn ok(msg: &str) {
    println!("{}", msg);
}

pub fn warn(msg: &str) {
    eprintln!("{}{}", "Warning: ".yellow().bold(), msg.yellow());
}

pub fn fatal(msg: &str) {
    eprintln!("{}{}", "Error: ".red().bold(), msg.bold());
}

pub fn info(msg: &str) {
    println!("{}", msg);
}

// ─────────────────────────────────────────────────────────────
//  Transaction table
//
//  ================================================================================
//   Package          Arch      Version              Repository          Size
//  ================================================================================
//  Installing:
//   vim              x86_64    9.1.0-1              debian-main         1.6 M
// ─────────────────────────────────────────────────────────────

pub fn print_transaction_table(plan: &TransactionPlan, arch: &str) {
    let w      = term_width();
    let bar    = "=".repeat(w);
    let name_w = 28usize;
    let arch_w = 9usize;
    let ver_w  = 26usize;
    let repo_w = 18usize;

    println!("{}", bar.bold());
    println!(
        " {:<name_w$} {:<arch_w$} {:<ver_w$} {:<repo_w$} {}",
        "Package".bold(),
             "Arch".bold(),
             "Version".bold(),
             "Repository".bold(),
             "Size".bold(),
    );
    println!("{}", bar.bold());

    if !plan.to_install.is_empty() {
        println!("{}", "Installing:".bold().green());
        for pkg in &plan.to_install {
            print_pkg_row(pkg, arch, name_w, arch_w, ver_w, repo_w);
        }
    }

    if !plan.to_upgrade.is_empty() {
        println!("{}", "Upgrading:".bold().yellow());
        for pkg in &plan.to_upgrade {
            let old = plan.upgrade_from.get(&pkg.name).map(|s| s.as_str()).unwrap_or("?");
            print_upgrade_row(pkg, old, arch, name_w, arch_w, ver_w, repo_w);
        }
    }

    if !plan.to_remove.is_empty() {
        println!("{}", "Removing:".bold().red());
        for name in &plan.to_remove {
            println!(
                " {:<name_w$} {:<arch_w$}",
                name.red(),
                     arch.dimmed(),
            );
        }
    }

    if !plan.to_autoremove.is_empty() {
        println!("{}", "Removing unused dependencies:".bold().red());
        for name in &plan.to_autoremove {
            println!(
                " {:<name_w$} {:<arch_w$}",
                name.red(),
                     arch.dimmed(),
            );
        }
    }

    println!();
}

fn repo_short(pkg: &Package) -> &str {
    pkg.repo_base_uri
    .as_deref()
    .unwrap_or("unknown")
    .trim_end_matches('/')
    .split('/')
    .last()
    .unwrap_or("unknown")
}

fn print_pkg_row(pkg: &Package, arch: &str, nw: usize, aw: usize, vw: usize, rw: usize) {
    let repo     = repo_short(pkg);
    let size_str = pkg.download_size.map(human_size).unwrap_or_else(|| "?".to_string());

    println!(
        " {:<nw$} {:<aw$} {:<vw$} {:<rw$} {}",
        truncate(&pkg.name, nw).green(),
             truncate(arch, aw).cyan(),
             truncate(&pkg.version, vw).bright_white(),
             truncate(repo, rw).dimmed(),
             size_str.yellow(),
    );
}

fn print_upgrade_row(pkg: &Package, old: &str, arch: &str, nw: usize, aw: usize, vw: usize, rw: usize) {
    let repo        = repo_short(pkg);
    let size_str    = pkg.download_size.map(human_size).unwrap_or_else(|| "?".to_string());
    let ver_display = format!("{} -> {}", old, pkg.version);

    println!(
        " {:<nw$} {:<aw$} {:<vw$} {:<rw$} {}",
        truncate(&pkg.name, nw).yellow(),
             truncate(arch, aw).cyan(),
             truncate(&ver_display, vw).bright_white(),
             truncate(repo, rw).dimmed(),
             size_str.yellow(),
    );
}

// ─────────────────────────────────────────────────────────────
//  Transaction Summary
//
//  Transaction Summary
//  ================================================================================
//  Install   3 Packages
//  Upgrade   1 Package
//
//  Total download size: 8.7 M
//  Installed size: 28 M
//  Is this ok [y/N]:
// ─────────────────────────────────────────────────────────────

pub fn print_transaction_summary(plan: &TransactionPlan) {
    let w = term_width();
    println!();
    println!("{}", "Transaction Summary".bold());
    println!("{}", "=".repeat(w).bold());

    if !plan.to_install.is_empty() {
        let n = plan.to_install.len();
        println!("{:<9} {} Package{}", "Install".green().bold(), n.to_string().bold(), plural(n));
    }
    if !plan.to_upgrade.is_empty() {
        let n = plan.to_upgrade.len();
        println!("{:<9} {} Package{}", "Upgrade".yellow().bold(), n.to_string().bold(), plural(n));
    }
    if !plan.to_remove.is_empty() {
        let n = plan.to_remove.len();
        println!("{:<9} {} Package{}", "Remove".red().bold(), n.to_string().bold(), plural(n));
    }
    if !plan.to_autoremove.is_empty() {
        let n = plan.to_autoremove.len();
        println!("{:<9} {} Package{}", "Remove".red().bold(), n.to_string().bold(), plural(n));
    }

    println!();

    if plan.download_bytes > 0 {
        println!("Total download size: {}", human_size(plan.download_bytes).yellow().bold());
    }
    if plan.install_bytes > 0 {
        println!("Installed size: {}", human_size(plan.install_bytes).yellow().bold());
    }
    if plan.freed_bytes > 0 {
        println!("Freed space: {}", human_size(plan.freed_bytes).yellow().bold());
    }

    if !plan.warnings.is_empty() {
        println!();
        for w in &plan.warnings {
            warn(w);
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// ─────────────────────────────────────────────────────────────
//  Confirm prompt
// ─────────────────────────────────────────────────────────────

pub fn confirm(prompt: &str) -> io::Result<bool> {
    print!("{} [{}]: ", prompt.bold(), "y/N".bold());
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    let t = s.trim().to_lowercase();
    Ok(t == "y" || t == "yes")
}

// ─────────────────────────────────────────────────────────────
//  Download progress bars
// ─────────────────────────────────────────────────────────────

pub fn make_download_bars(packages: &[Package]) -> (MultiProgress, Vec<ProgressBar>) {
    let mp = MultiProgress::new();

    let bar_style = ProgressStyle::with_template(
        "({pos}/{len}): {prefix:<40.bold} {bytes_per_sec:>12} | {bytes:>8}  {elapsed}",
    )
    .unwrap();

    let mut bars = Vec::new();
    for pkg in packages {
        let pb = mp.add(ProgressBar::new(pkg.download_size.unwrap_or(0)));
        pb.set_style(bar_style.clone());
        let label = format!("{}-{}", pkg.name, pkg.version);
        pb.set_prefix(truncate_owned(label, 38));
        pb.set_length(pkg.download_size.unwrap_or(1));
        bars.push(pb);
    }

    (mp, bars)
}

pub fn make_overall_bar(total_bytes: u64) -> ProgressBar {
    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "Overall [{bar:50.cyan/white}] {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
        )
        .unwrap()
        .progress_chars("━━─"),
    );
    pb
}

// ─────────────────────────────────────────────────────────────
//  Running transaction steps  (DNF-identical)
// ─────────────────────────────────────────────────────────────

pub fn print_running_transaction() {
    println!("{}", "Running transaction check".bold());
    println!("{}", "Transaction check succeeded.".bold());
    println!("{}", "Running transaction test".bold());
    println!("{}", "Transaction test succeeded.".bold());
    println!("{}", "Running transaction".bold());
}

pub fn print_install_step(action: &str, pkg_label: &str, current: usize, total: usize) {
    println!(
        "  {:<18}: {:<52} {}/{}",
        action.bold(),
             pkg_label.green(),
             current,
             total
    );
}

pub fn print_verify_step(pkg_label: &str, current: usize, total: usize) {
    println!(
        "  {:<18}: {:<52} {}/{}",
        "Verifying".bold(),
             pkg_label.dimmed(),
             current,
             total
    );
}

pub fn print_remove_step(pkg_label: &str, current: usize, total: usize) {
    println!(
        "  {:<18}: {:<52} {}/{}",
        "Erasing".bold(),
             pkg_label.red(),
             current,
             total
    );
}

// ─────────────────────────────────────────────────────────────
//  Post-transaction summaries
// ─────────────────────────────────────────────────────────────

pub fn print_installed_summary(pkgs: &[Package]) {
    if pkgs.is_empty() { return; }
    println!();
    println!("{}:", "Installed".green().bold());
    for p in pkgs {
        println!("  {}.{} {}", p.name.bold(), p.architecture.dimmed(), p.version.cyan());
    }
}

pub fn print_upgraded_summary(
    pkgs: &[Package],
    from: &std::collections::HashMap<String, String>,
) {
    if pkgs.is_empty() { return; }
    println!();
    println!("{}:", "Upgraded".yellow().bold());
    for p in pkgs {
        let old = from.get(&p.name).map(|s| s.as_str()).unwrap_or("?");
        println!(
            "  {}.{} {} -> {}",
            p.name.bold(),
                 p.architecture.dimmed(),
                 old.dimmed(),
                 p.version.cyan()
        );
    }
}

pub fn print_removed_summary(names: &[String]) {
    if names.is_empty() { return; }
    println!();
    println!("{}:", "Removed".red().bold());
    for n in names {
        println!("  {}", n.bold());
    }
}

// ─────────────────────────────────────────────────────────────
//  Search results
// ─────────────────────────────────────────────────────────────

pub fn print_search_header(query: &str, count: usize) {
    let w = term_width();
    println!(
        "{}",
        format!("==================== N/S matched: {} : {} ====================", count, query)
            .bold()
    );
    println!(
        " {:<42} {:<24} {}",
        "Name and summary".bold(),
             "Version".bold(),
             "Repo".bold()
    );
    println!("{}", "─".repeat(w).dimmed());
}

pub fn print_search_result(pkg: &Package, is_installed: bool) {
    let installed_tag = if is_installed {
        format!(" {}", "@".green().bold())
    } else {
        String::new()
    };

    println!(
        "{}{}.{}  {}",
        pkg.name.bold().bright_white(),
             installed_tag,
             pkg.architecture.dimmed(),
             pkg.version.cyan()
    );

    if let Some(ref d) = pkg.description_short {
        println!("  {}", d.dimmed());
    }
}

// ─────────────────────────────────────────────────────────────
//  Package info
// ─────────────────────────────────────────────────────────────

pub fn print_package_info(pkg: &Package, is_installed: bool, installed_ver: Option<&str>) {
    let w = term_width();
    println!("{}", "=".repeat(w).dimmed());

    let field = |label: &str, value: &str| {
        println!("{:<20}: {}", label.bold(), value);
    };

    field("Name",         &pkg.name);
    field("Epoch",        "0");
    field("Version",      &pkg.version);
    field("Architecture", &pkg.architecture);

    if is_installed {
        field("Status", &"Installed".green().bold().to_string());
        if let Some(v) = installed_ver {
            if v != pkg.version {
                field("Installed version", v);
            }
        }
    } else {
        field("Status", &"Available".yellow().to_string());
    }

    if let Some(ref s) = pkg.section    { field("Section",    s); }
    if let Some(ref m) = pkg.maintainer { field("Maintainer", m); }

    if let Some(sz) = pkg.installed_size_kb {
        field("Size", &human_size(sz * 1024));
    }
    if let Some(sz) = pkg.download_size {
        field("Download size", &human_size(sz));
    }
    if let Some(ref h) = pkg.homepage { field("URL", h); }

    let repo = repo_short(pkg);
    field("Repo", repo);

    if let Some(ref d) = pkg.depends    { field("Requires",   d); }
    if let Some(ref r) = pkg.recommends { field("Recommends", r); }
    if let Some(ref c) = pkg.conflicts  { field("Conflicts",  c); }

    println!("{}", "=".repeat(w).dimmed());

    if let Some(ref d) = pkg.description_short {
        field("Summary", d);
    }
    if let Some(ref d) = pkg.description_long {
        println!("{:<20}:", "Description".bold());
        for line in d.lines() {
            let line = line.trim();
            if line == "." {
                println!();
            } else {
                println!("  {}", line);
            }
        }
    }

    println!("{}", "=".repeat(w).dimmed());
}

// ─────────────────────────────────────────────────────────────
//  List entries
// ─────────────────────────────────────────────────────────────

pub fn print_list_entry(
    name:         &str,
    version:      &str,
    arch:         &str,
    is_installed: bool,
    repo:         &str,
    new_version:  Option<&str>,
) {
    let repo_tag = if is_installed {
        if let Some(nv) = new_version {
            format!("@installed (upgrade available: {})", nv.yellow())
        } else {
            "@installed".green().bold().to_string()
        }
    } else {
        repo.dimmed().to_string()
    };

    println!(
        "{:<40} {:<26} {}",
        format!("{}.{}", name.bold(), arch.dimmed()),
            version.cyan(),
             repo_tag
    );
}

// ─────────────────────────────────────────────────────────────
//  History table
// ─────────────────────────────────────────────────────────────

pub fn print_history(entries: &[HistoryEntry]) {
    let w = term_width();
    println!("{}", "=".repeat(w).dimmed());
    println!(
        "{:<6}  {:<22}  {:<12}  {:<30}  {}",
        "ID".bold(),
             "Command line".bold(),
             "Action".bold(),
             "Package".bold(),
             "Date and time".bold()
    );
    println!("{}", "=".repeat(w).dimmed());

    for e in entries {
        let action_str = match e.action.as_str() {
            "install" => format!("{:<12}", "Install".green().bold()),
            "remove"  => format!("{:<12}", "Erase".red().bold()),
            "upgrade" => format!("{:<12}", "Upgrade".yellow().bold()),
            other     => format!("{:<12}", other),
        };

        let pkg_ver = match (&e.old_ver, &e.new_ver) {
            (None,       Some(nv)) => nv.clone(),
            (Some(ov),   None    ) => ov.clone(),
            (Some(ov), Some(nv)  ) => format!("{} -> {}", ov.dimmed(), nv.bright_cyan()),
            _                      => String::new(),
        };

        let date_str = e.timestamp.format("%Y-%m-%d %H:%M").to_string();

        println!(
            "{:<6}  {:<22}  {}  {:<38}  {}",
            e.id.to_string().dimmed(),
                 format!("lpm {}", e.action).dimmed(),
                     action_str,
                 format!("{} {}", e.package.bold(), pkg_ver),
                     date_str.dimmed()
        );
    }

    println!("{}", "=".repeat(w).dimmed());
}

// ─────────────────────────────────────────────────────────────
//  Update / makecache spinner
// ─────────────────────────────────────────────────────────────

pub fn make_repo_spinner(label: &str, mp: &MultiProgress) -> ProgressBar {
    let pb = mp.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan}  {prefix:<40.bold}  {wide_msg}")
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_prefix(label.to_owned());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

// ─────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────

/// Human-readable size matching DNF style: "1.6 M", "800 k", "42 B"
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} M", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.0} k", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

fn truncate_owned(s: String, max: usize) -> String {
    if s.len() <= max { s } else { s[..max].to_string() }
}
