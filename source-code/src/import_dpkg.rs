use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

use crate::db::{InstalledDb, InstallReason};
use crate::package::Package;

pub const DPKG_STATUS: &str = "/var/lib/dpkg/status";
pub const DPKG_INFO_DIR: &str = "/var/lib/dpkg/info";

/// Importuje wszystkie pakiety z dpkg/status do lpm.db
pub fn import_from_dpkg() -> Result<()> {
    println!("Importing packages from dpkg...");

    let status_path = Path::new(DPKG_STATUS);
    if !status_path.exists() {
        bail!("dpkg status file not found at {}", DPKG_STATUS);
    }

    let content = fs::read_to_string(status_path)
    .context("Failed to read dpkg status file")?;

    let packages = parse_dpkg_status(&content)?;
    let db = InstalledDb::open()?;

    let mut imported = 0;
    let mut skipped = 0;

    for pkg in packages {
        if db.is_installed(&pkg.name) {
            skipped += 1;
            continue;
        }

        let files = read_package_files(&pkg.name);

        db.record_install(
            &pkg,
            InstallReason::User,
            &files,
        )?;

        imported += 1;
        if imported % 100 == 0 {
            print!(".");
            std::io::Write::flush(&mut std::io::stdout())?;
        }
    }

    println!("\n\nImport completed:");
    println!("  ✅ Imported: {} packages", imported);
    println!("  ⏭️  Skipped (already in lpm.db): {}", skipped);
    println!("  📁 Database: /var/lib/lpm/lpm.db");

    Ok(())
}

fn parse_dpkg_status(content: &str) -> Result<Vec<Package>> {
    let mut packages = Vec::new();

    for block in content.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }

        let status = get_field(block, "Status").unwrap_or_default();
        if !status.contains("installed") {
            continue;
        }

        let name = match get_field(block, "Package") {
            Some(n) => n,
            None => continue,
        };

        let version = get_field(block, "Version").unwrap_or_default();
        let arch = get_field(block, "Architecture").unwrap_or_else(|| "all".to_string());
        let size_kb: u64 = get_field(block, "Installed-Size")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

        let depends = get_field(block, "Depends");
        let recommends = get_field(block, "Recommends");
        let section = get_field(block, "Section");
        let maintainer = get_field(block, "Maintainer");
        let description = get_field(block, "Description");

        let (desc_short, desc_long) = split_description(description.as_deref());

        packages.push(Package {
            name,
            version,
            architecture: arch,
            description_short: desc_short,
            description_long: desc_long,
            section,
            priority: None,
            maintainer,
            installed_size_kb: Some(size_kb),
                      download_size: None,
                      filename: None,
                      sha256: None,
                      md5sum: None,
                      depends,
                      pre_depends: None,
                      recommends,
                      suggests: None,
                      conflicts: None,
                      replaces: None,
                      breaks: None,
                      provides: None,
                      homepage: None,
                      source: None,
                      repo_base_uri: None,
        });
    }

    Ok(packages)
}

fn read_package_files(pkg_name: &str) -> Vec<String> {
    let list_path = Path::new(DPKG_INFO_DIR).join(format!("{}.list", pkg_name));
    let content = match fs::read_to_string(&list_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    content
    .lines()
    .filter(|line| !line.is_empty())
    .map(|line| line.to_string())
    .collect()
}

fn get_field(block: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    block
    .lines()
    .find(|l| l.to_lowercase().starts_with(&prefix.to_lowercase()))
    .map(|l| l[prefix.len()..].trim().to_owned())
}

fn split_description(raw: Option<&str>) -> (Option<String>, Option<String>) {
    match raw {
        None => (None, None),
        Some(s) => {
            let mut lines = s.lines();
            let short = lines.next().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty());
            let long: Vec<&str> = lines.collect();
            let long_str = long.join("\n");
            (short, if long_str.trim().is_empty() { None } else { Some(long_str) })
        }
    }
}
