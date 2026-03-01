/// Filesystem-level install and remove logic.
///
/// Installs .deb packages by:
///   1. Running preinst maintainer script (if present)
///   2. Extracting data.tar to /
///   3. Running postinst maintainer script (if present)
///   4. Recording the install in our SQLite DB
///
/// Remove:
///   1. Deleting tracked files from the DB file list
///   2. Cleaning up empty directories
///   3. Optionally purging config files
///   4. Removing the DB record

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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

    // ── preinst ───────────────────────────────────────────────
    let script_arg = if job.is_upgrade {
        job.old_version.as_deref().unwrap_or("")
    } else {
        ""
    };
    let preinst_action = if job.is_upgrade { "upgrade" } else { "install" };
    run_maintainer_script(&job.deb, "preinst", &[preinst_action, script_arg]);

    // ── Extract data files ────────────────────────────────────
    let written = job.deb.extract_data(root)
    .with_context(|| format!("Extracting data from {}", pkg.name))?;

    let file_paths: Vec<String> = written
    .iter()
    .map(|p: &PathBuf| p.to_string_lossy().to_string())
    .collect();

    // ── Fix permissions on bin files ─────────────────────────
    fix_permissions(&written);

    // ── postinst ─────────────────────────────────────────────
    let postinst_action = if job.is_upgrade { "configure" } else { "configure" };
    run_maintainer_script(&job.deb, "postinst", &[postinst_action, script_arg]);

    // ── Record in DB ─────────────────────────────────────────
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

    // ── Delete files ──────────────────────────────────────────
    let mut removed = 0usize;
    let mut _errors  = 0usize;

    for f in &files {
        let path = Path::new(f);
        if !path.exists() {
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(_) => removed += 1,
            Err(e) => {
                eprintln!("    {} removing {:?}: {}", "warn".yellow(), path, e);
                _errors += 1;
            }
        }
    }

    // ── Clean up empty dirs (deepest first) ───────────────────
    let dirs: std::collections::BTreeSet<PathBuf> = files
    .iter()
    .filter_map(|f| Path::new(f).parent().map(|p| p.to_owned()))
    .collect();

    let mut dir_vec: Vec<PathBuf> = dirs.into_iter().collect();
    dir_vec.sort_by(|a, b| b.cmp(a)); // reverse = deepest first

    for dir in dir_vec {
        if is_safe_to_rmdir(&dir) {
            let _ = std::fs::remove_dir(&dir);
        }
    }

    // ── Purge config files ────────────────────────────────────
    if purge {
        purge_config_files(&installed.name);
    }

    // ── Remove DB record ──────────────────────────────────────
    db.record_remove(&installed.name, &installed.version)?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  Internal helpers
// ─────────────────────────────────────────────────────────────

/// Run a maintainer script from the .deb control tarball (best-effort).
/// TODO v2: cache scripts during extract so we can run them on remove too.
fn run_maintainer_script(_deb: &DebPackage, _script: &str, _args: &[&str]) {
    // Currently a no-op placeholder.
    // Full implementation would extract the script from control.tar,
    // write it to a temp file, chmod +x, and execute it.
}

fn fix_permissions(paths: &[PathBuf]) {
    for path in paths {
        // Set executable bit on files inside bin/sbin/libexec dirs
        let is_bin = path.ancestors().any(|a| {
            matches!(
                a.file_name().and_then(|n| n.to_str()),
                     Some("bin") | Some("sbin") | Some("libexec")
            )
        });

        if is_bin {
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perms = meta.permissions();
                let mode = perms.mode();
                // Where owner can read, set owner can execute; same for group/other
                let exec_bits = (mode & 0o444) >> 2;
                perms.set_mode(mode | exec_bits);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
}

fn is_safe_to_rmdir(path: &Path) -> bool {
    let protected: &[&str] = &[
        "/",
        "/usr", "/usr/bin", "/usr/lib", "/usr/lib64",
        "/usr/share", "/usr/include", "/usr/local",
        "/bin", "/sbin", "/lib", "/lib64",
        "/etc", "/var", "/var/lib", "/var/cache", "/var/log",
        "/tmp", "/opt", "/home", "/root",
        "/sys", "/proc", "/dev", "/run",
    ];

    let s = path.to_string_lossy();
    if protected.contains(&s.as_ref()) {
        return false;
    }
    // Require depth > 4 so we never accidentally remove e.g. /usr/share/foo
    path.components().count() > 4
}

fn purge_config_files(pkg_name: &str) {
    let etc_path = PathBuf::from("/etc").join(pkg_name);
    if etc_path.exists() && etc_path.is_dir() {
        let _ = std::fs::remove_dir_all(&etc_path);
    }
}
