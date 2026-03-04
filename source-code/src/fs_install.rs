use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::alternatives::{fix_alternatives, needs_ldconfig, run_ldconfig};
use crate::db::{InstalledDb, InstalledPackage, InstallReason};
use crate::deb::DebPackage;
use crate::dpkg_status;
use crate::log;
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

    log::pkg(
        if job.is_upgrade { "upgrade" } else { "install" },
            &pkg.name, &pkg.version,
    );

    let old_ver    = job.old_version.as_deref().unwrap_or("");
    let action_arg = if job.is_upgrade { "upgrade" } else { "install" };

    // ── preinst ───────────────────────────────────────────────
    run_maintainer_script(&job.deb, "preinst", &[action_arg, old_ver]);

    // ── Extract data.tar ──────────────────────────────────────
    let (written, all_paths) = job.deb.extract_data(root)
    .with_context(|| format!("Extracting {}", pkg.name))?;

    log::info(&format!("extracted {} files for {}", written.len(), pkg.name));

    // ── Record in lpm DB first (before postinst) ─────────────
    // This way if postinst calls dpkg-query it finds our package
    let file_paths: Vec<String> = written
    .iter()
    .map(|p| p.to_string_lossy().to_string())
    .collect();

    if job.is_upgrade {
        db.record_upgrade(old_ver, pkg, &file_paths)?;
    } else {
        db.record_install(pkg, job.reason, &file_paths)?;
    }

    // ── Sync to /var/lib/dpkg/status ─────────────────────────
    // This makes dpkg-query, py3compile, dpkg-maintscript-helper
    // aware of the package BEFORE postinst runs.
    dpkg_status::record_in_dpkg(
        &pkg.name,
        &pkg.version,
        &pkg.architecture,
        pkg.installed_size_kb.unwrap_or(0),
                                pkg.depends.as_deref(),
                                pkg.recommends.as_deref(),
                                pkg.section.as_deref(),
                                pkg.maintainer.as_deref(),
                                pkg.description_short.as_deref(),
                                &file_paths,
    );

    // ── postinst ──────────────────────────────────────────────
    // Run AFTER dpkg/status is updated so dpkg-query works inside postinst.
    let postinst_ran = run_maintainer_script(&job.deb, "postinst", &["configure", old_ver]);

    // ── fix_alternatives fallback ─────────────────────────────
    // If postinst didn't run or didn't call update-alternatives, fix manually.
    if !postinst_ran {
        fix_alternatives(&all_paths);
    }

    // ── Permissions + ldconfig ────────────────────────────────
    fix_permissions(&written);

    if needs_ldconfig(&written) {
        log::info("running ldconfig");
        run_ldconfig();
    }

    if job.is_upgrade {
        log::info(&format!("upgraded {} {} -> {}", pkg.name, old_ver, pkg.version));
    } else {
        log::info(&format!("installed {}-{}", pkg.name, pkg.version));
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  Remove
// ─────────────────────────────────────────────────────────────

pub fn remove_package(installed: &InstalledPackage, db: &InstalledDb, purge: bool) -> Result<()> {
    log::pkg("remove", &installed.name, &installed.version);

    let files = db.files_of(&installed.name);

    // Remove /etc/alternatives pointing to our files
    remove_alternatives_for(&files);

    // Delete files
    for f in &files {
        let path = Path::new(f);
        if path.is_dir() { continue; }
        if path.symlink_metadata().is_err() { continue; }

        match std::fs::remove_file(path) {
            Ok(_)  => log::file_op("delete", f),
            Err(e) => {
                let msg = format!("removing {:?}: {}", path, e);
                log::warn(&msg);
                eprintln!("    {} {}", "warn".yellow().dimmed(), msg);
            }
        }
    }

    // Clean empty dirs (deepest first)
    let mut dirs: Vec<PathBuf> = files
    .iter()
    .filter_map(|f| Path::new(f).parent().map(|p| p.to_owned()))
    .collect::<std::collections::BTreeSet<_>>()
    .into_iter()
    .collect();
    dirs.sort_by(|a, b| b.cmp(a));

    for dir in dirs {
        if is_safe_to_rmdir(&dir) {
            if std::fs::remove_dir(&dir).is_ok() {
                log::file_op("rmdir", &dir.to_string_lossy());
            }
        }
    }

    if purge { purge_config_files(&installed.name); }

    // Remove from lpm DB
    db.record_remove(&installed.name, &installed.version)?;

    // Remove from /var/lib/dpkg/status
    dpkg_status::remove_from_dpkg(&installed.name);

    log::info(&format!("removed {}-{}", installed.name, installed.version));
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  Maintainer script execution
// ─────────────────────────────────────────────────────────────

fn run_maintainer_script(deb: &DebPackage, script_name: &str, args: &[&str]) -> bool {
    let content = match deb.extract_script(script_name) {
        Some(s) => s,
        None    => return false,
    };

    let tmp_path = format!("/tmp/lpm-{}-{}", deb.control.name, script_name);

    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(content.as_bytes())?;
        let mut perms = f.metadata()?.permissions();
        perms.set_mode(0o755);
        f.set_permissions(perms)?;
        Ok(())
    })();

    if let Err(e) = write_result {
        log::warn(&format!("could not write {}: {}", script_name, e));
        return false;
    }

    log::info(&format!("running {} for {}", script_name, deb.control.name));

    let status = std::process::Command::new(&tmp_path)
    .args(args)
    // Required env vars for dpkg-maintscript-helper and friends
    .env("DPKG_MAINTSCRIPT_PACKAGE",   &deb.control.name)
    .env("DPKG_MAINTSCRIPT_ARCH",      &deb.control.architecture)
    .env("DPKG_MAINTSCRIPT_NAME",      script_name)
    .env("DPKG_RUNNING_VERSION",       "1.23.5")
    .env("DEBIAN_FRONTEND",            "noninteractive")
    .env("DEBCONF_NONINTERACTIVE_SEEN","true")
    // Prevent systemd unit activation during install
    .env("DPKG_NO_TSTP",              "1")
    .status();

    let _ = std::fs::remove_file(&tmp_path);

    match status {
        Ok(s) => {
            if !s.success() {
                log::warn(&format!(
                    "{} for {} exited {:?}", script_name, deb.control.name, s.code()
                ));
            }
            true
        }
        Err(e) => {
            log::warn(&format!("failed to run {}: {}", script_name, e));
            false
        }
    }
}

// ─────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────

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
                perms.set_mode(mode | ((mode & 0o444) >> 2));
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
}

fn remove_alternatives_for(files: &[String]) {
    let alt_dir = Path::new("/etc/alternatives");
    if !alt_dir.exists() { return; }

    let file_set: std::collections::HashSet<&str> =
    files.iter().map(|s| s.as_str()).collect();

    if let Ok(entries) = std::fs::read_dir(alt_dir) {
        for entry in entries.flatten() {
            let alt_path = entry.path();
            if let Ok(target) = std::fs::read_link(&alt_path) {
                if file_set.contains(target.to_string_lossy().as_ref()) {
                    if std::fs::remove_file(&alt_path).is_ok() {
                        log::file_op("rm-alt", &alt_path.to_string_lossy());
                    }
                }
            }
        }
    }
}

const PROTECTED: &[&str] = &[
    "/", "/usr", "/usr/bin", "/usr/lib", "/usr/lib64", "/usr/libexec",
"/usr/share", "/usr/include", "/usr/local",
"/usr/share/doc", "/usr/share/man", "/usr/share/info", "/usr/share/locale",
"/bin", "/sbin", "/lib", "/lib64",
"/etc", "/var", "/var/lib", "/var/cache", "/var/log",
"/tmp", "/opt", "/home", "/root", "/sys", "/proc", "/dev", "/run",
];

fn is_safe_to_rmdir(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if PROTECTED.contains(&s.as_ref()) { return false; }
    path.components().count() > 4
}

fn purge_config_files(pkg_name: &str) {
    let p = PathBuf::from("/etc").join(pkg_name);
    if p.exists() && p.is_dir() {
        if std::fs::remove_dir_all(&p).is_ok() {
            log::file_op("purge", &p.to_string_lossy());
        }
    }
}
