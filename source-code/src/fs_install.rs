use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::alternatives::{fix_alternatives, needs_ldconfig, run_ldconfig};
use crate::db::{InstalledDb, InstalledPackage, InstallReason};
use crate::deb::DebPackage;
use crate::package::Package;

pub const INSTALL_ROOT: &str = "/";

// ─────────────────────────────────────────────────────────────
//  Install
// ─────────────────────────────────────────────────────────────

pub struct InstallJob {
    pub pkg:         Package,
    pub deb:         DebPackage,
    pub path:        PathBuf,
    pub reason:      InstallReason,
    pub is_upgrade:  bool,
    pub old_version: Option<String>,
}

pub fn install_package(job: &InstallJob, db: &InstalledDb) -> Result<()> {
    let root = Path::new(INSTALL_ROOT);
    let pkg  = &job.pkg;

    let script_arg     = job.old_version.as_deref().unwrap_or("");
    let preinst_action = if job.is_upgrade { "upgrade" } else { "install" };
    run_maintainer_script(&job.deb, "preinst", &[preinst_action, script_arg]);

    // Extract all files (Regular + Symlinks + HardLinks)
    // `written` contains only regular files and hard links (not dirs, not symlinks)
    let (written, all_paths) = job.deb.extract_data(root)
    .with_context(|| format!("Extracting data from {}", pkg.name))?;

    // Fix /etc/alternatives/* broken symlinks that postinst would have created
    fix_alternatives(&all_paths);

    // Set executable permissions on bin files
    fix_permissions(&written);

    // Run ldconfig if we installed shared libraries
    if needs_ldconfig(&written) {
        run_ldconfig();
    }

    run_maintainer_script(&job.deb, "postinst", &["configure", script_arg]);

    // Only track regular files in DB (for clean removal)
    let file_paths: Vec<String> = written
    .iter()
    .map(|p| p.to_string_lossy().to_string())
    .collect();

    if job.is_upgrade {
        let old = job.old_version.as_deref().unwrap_or("0");
        db.record_upgrade(old, pkg, &file_paths)?;
    } else {
        db.record_install(pkg, job.reason, &file_paths)?;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  Remove
// ─────────────────────────────────────────────────────────────

pub fn remove_package(installed: &InstalledPackage, db: &InstalledDb, purge: bool) -> Result<()> {
    let files = db.files_of(&installed.name);

    // Also remove any /etc/alternatives entries we may have created
    remove_alternatives_for(&files);

    // Delete regular files and symlinks
    for f in &files {
        let path = Path::new(f);

        if path.is_dir() {
            continue; // never remove directories here
        }
        // exists() follows symlinks; symlink_metadata() doesn't
        if !path.symlink_metadata().is_ok() {
            continue; // doesn't exist at all
        }

        if let Err(e) = std::fs::remove_file(path) {
            eprintln!("    {} removing {:?}: {}", "warn".yellow().dimmed(), path, e);
        }
    }

    // Clean up now-empty package-specific dirs (deepest first)
    let dirs: std::collections::BTreeSet<PathBuf> = files
    .iter()
    .filter_map(|f| Path::new(f).parent().map(|p| p.to_owned()))
    .collect();

    let mut dir_vec: Vec<PathBuf> = dirs.into_iter().collect();
    dir_vec.sort_by(|a, b| b.cmp(a));

    for dir in dir_vec {
        if is_safe_to_rmdir(&dir) {
            let _ = std::fs::remove_dir(&dir);
        }
    }

    if purge {
        purge_config_files(&installed.name);
    }

    db.record_remove(&installed.name, &installed.version)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────

fn run_maintainer_script(_deb: &DebPackage, _script: &str, _args: &[&str]) {
    // TODO v2: extract and execute preinst/postinst/prerm/postrm
}

fn fix_permissions(paths: &[PathBuf]) {
    for path in paths {
        let is_bin = path.ancestors().any(|a| {
            matches!(
                a.file_name().and_then(|n| n.to_str()),
                     Some("bin") | Some("sbin") | Some("libexec")
            )
        });
        if is_bin {
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perms = meta.permissions();
                let mode      = perms.mode();
                let exec_bits = (mode & 0o444) >> 2;
                perms.set_mode(mode | exec_bits);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
}

/// Remove /etc/alternatives/* entries that point to files we're about to remove.
fn remove_alternatives_for(files: &[String]) {
    let alt_dir = Path::new("/etc/alternatives");
    if !alt_dir.exists() { return; }

    let file_set: std::collections::HashSet<&str> =
    files.iter().map(|s| s.as_str()).collect();

    if let Ok(entries) = std::fs::read_dir(alt_dir) {
        for entry in entries.flatten() {
            let alt_path = entry.path();
            // Read where this alternative points
            if let Ok(target) = std::fs::read_link(&alt_path) {
                let target_str = target.to_string_lossy().to_string();
                if file_set.contains(target_str.as_str()) {
                    let _ = std::fs::remove_file(&alt_path);
                }
            }
        }
    }
}

const PROTECTED: &[&str] = &[
    "/",
"/usr", "/usr/bin", "/usr/lib", "/usr/lib64", "/usr/libexec",
"/usr/share", "/usr/include", "/usr/local",
"/usr/share/doc", "/usr/share/man", "/usr/share/info",
"/usr/share/locale",
"/bin", "/sbin", "/lib", "/lib64",
"/etc", "/var", "/var/lib", "/var/cache", "/var/log",
"/tmp", "/opt", "/home", "/root",
"/sys", "/proc", "/dev", "/run",
];

fn is_safe_to_rmdir(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if PROTECTED.contains(&s.as_ref()) { return false; }
    path.components().count() > 4
}

fn purge_config_files(pkg_name: &str) {
    let etc_path = PathBuf::from("/etc").join(pkg_name);
    if etc_path.exists() && etc_path.is_dir() {
        let _ = std::fs::remove_dir_all(&etc_path);
    }
}
