use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::Path;

pub const DPKG_STATUS:    &str = "/var/lib/dpkg/status";
pub const DPKG_INFO_DIR:  &str = "/var/lib/dpkg/info";
pub const DPKG_UPDATES:   &str = "/var/lib/dpkg/updates";
pub const DPKG_AVAILABLE: &str = "/var/lib/dpkg/available";

/// Write a single package entry to /var/lib/dpkg/status.
/// If the package is already there, update the version.
/// Called immediately after install_package() succeeds.
pub fn record_in_dpkg(
    name:         &str,
    version:      &str,
    arch:         &str,
    size_kb:      u64,
    depends:      Option<&str>,
    recommends:   Option<&str>,
    section:      Option<&str>,
    maintainer:   Option<&str>,
    description:  Option<&str>,
    files:        &[String],
) {
    // Ensure dpkg dirs exist
    let _ = std::fs::create_dir_all(DPKG_INFO_DIR);
    let _ = std::fs::create_dir_all(DPKG_UPDATES);

    // Read current status file
    let current = std::fs::read_to_string(DPKG_STATUS).unwrap_or_default();

    // Parse into blocks keyed by package name
    let mut blocks: Vec<String> = current
    .split("\n\n")
    .filter(|b| !b.trim().is_empty())
    .map(|b| b.to_owned())
    .collect();

    // Build the new/updated block
    let new_block = build_status_block(
        name, version, arch, size_kb,
        depends, recommends, section, maintainer, description,
    );

    // Replace existing or append
    let existing = blocks.iter().position(|b| {
        b.lines().any(|l| {
            l.starts_with("Package:") &&
            l["Package:".len()..].trim() == name
        })
    });

    match existing {
        Some(i) => blocks[i] = new_block,
        None    => blocks.push(new_block),
    }

    // Write back
    let content = blocks.join("\n\n") + "\n\n";
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true).create(true).truncate(true)
        .open(DPKG_STATUS)
        {
            let _ = f.write_all(content.as_bytes());
        }

        // Write /var/lib/dpkg/info/<pkg>.list with file list
        write_dpkg_list(name, arch, files);

    // Write /var/lib/dpkg/info/<pkg>.md5sums (empty stub)
    let md5_path = format!("{}/{}.md5sums", DPKG_INFO_DIR, name);
    let _ = std::fs::write(&md5_path, "");
}

/// Remove a package from /var/lib/dpkg/status (mark as not-installed).
pub fn remove_from_dpkg(name: &str) {
    let current = match std::fs::read_to_string(DPKG_STATUS) {
        Ok(s)  => s,
        Err(_) => return,
    };

    let mut blocks: Vec<String> = current
    .split("\n\n")
    .filter(|b| !b.trim().is_empty())
    .map(|b| b.to_owned())
    .collect();

    // Mark as deinstalled rather than removing entirely
    // (dpkg convention — allows `dpkg --purge` later if needed)
    for block in &mut blocks {
        let is_pkg = block.lines().any(|l| {
            l.starts_with("Package:") &&
            l["Package:".len()..].trim() == name
        });
        if is_pkg {
            *block = block.lines().map(|l| {
                if l.starts_with("Status:") {
                    "Status: deinstall ok config-files".to_owned()
                } else {
                    l.to_owned()
                }
            }).collect::<Vec<_>>().join("\n");
        }
    }

    let content = blocks.join("\n\n") + "\n\n";
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true).create(true).truncate(true)
        .open(DPKG_STATUS)
        {
            let _ = f.write_all(content.as_bytes());
        }

        // Remove .list file
        let list_path = format!("{}/{}.list", DPKG_INFO_DIR, name);
    let _ = std::fs::remove_file(&list_path);
}

/// Read /var/lib/dpkg/status and return map of name → version
/// for all "install ok installed" packages.
pub fn read_dpkg_installed() -> HashMap<String, String> {
    let mut result = HashMap::new();
    let content = match std::fs::read_to_string(DPKG_STATUS) {
        Ok(s)  => s,
        Err(_) => return result,
    };

    for block in content.split("\n\n") {
        let block = block.trim();
        if block.is_empty() { continue; }

        let status = get_field(block, "Status").unwrap_or_default();
        if status != "install ok installed" { continue; }

        if let (Some(name), Some(ver)) = (
            get_field(block, "Package"),
                                          get_field(block, "Version"),
        ) {
            result.insert(name, ver);
        }
    }
    result
}

// ─────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────

fn build_status_block(
    name:        &str,
    version:     &str,
    arch:        &str,
    size_kb:     u64,
    depends:     Option<&str>,
    recommends:  Option<&str>,
    section:     Option<&str>,
    maintainer:  Option<&str>,
    description: Option<&str>,
) -> String {
    let mut s = String::new();
    s.push_str(&format!("Package: {}\n", name));
    s.push_str("Status: install ok installed\n");
    if let Some(sec) = section   { s.push_str(&format!("Section: {}\n", sec)); }
    if let Some(m)   = maintainer{ s.push_str(&format!("Maintainer: {}\n", m)); }
    s.push_str(&format!("Architecture: {}\n", arch));
    s.push_str(&format!("Version: {}\n", version));
    if let Some(d) = depends    { s.push_str(&format!("Depends: {}\n", d)); }
    if let Some(r) = recommends { s.push_str(&format!("Recommends: {}\n", r)); }
    s.push_str(&format!("Installed-Size: {}\n", size_kb));
    if let Some(desc) = description {
        s.push_str(&format!("Description: {}\n", desc));
    } else {
        s.push_str(&format!("Description: {}\n", name));
    }
    s
}

fn write_dpkg_list(name: &str, arch: &str, files: &[String]) {
    let list_path = format!("{}/{}.list", DPKG_INFO_DIR, name);
    let content: String = files
    .iter()
    .filter(|f| !f.is_empty())
    .map(|f| format!("{}\n", f))
    .collect();
    // Also add the directory entries derived from file paths
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for f in files {
        let p = Path::new(f);
        if let Some(parent) = p.parent() {
            let ps = parent.to_string_lossy().to_string();
            if !ps.is_empty() && ps != "/" {
                dirs.insert(ps);
            }
        }
    }
    let mut full = String::new();
    for d in dirs { full.push_str(&format!("{}\n", d)); }
    full.push_str(&content);
    let _ = std::fs::write(&list_path, full);
}

pub fn get_field(block: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    // Multi-line fields: only first line
    block.lines()
    .find(|l| l.to_lowercase().starts_with(&prefix.to_lowercase()))
    .map(|l| l[prefix.len()..].trim().to_owned())
}
