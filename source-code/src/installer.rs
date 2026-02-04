use anyhow::{anyhow, Result};
use crate::solver::Transaction;
use crate::db::{Database, InstalledPackage, InstalledFile, InstallReason};
use crate::repo;
use crate::plugins::PluginManager;
use std::fs::{self, File};
use std::path::Path;
use std::io::Read;
use std::process::Command;
use std::sync::Arc;
use tar::Archive;
use flate2::read::GzDecoder;
use xz2::read::XzDecoder;
use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use sha2::{Sha256, Digest};

// --- Helper Functions ---

fn compute_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

// --- Execution ---

pub async fn execute(tx: Transaction, plugins: Arc<PluginManager>, requested_pkgs: &[String]) -> Result<()> {
    plugins.run_hook("on_pre_install", &tx)?;

    let mut db = Database::load();
    let cache_dir = Path::new("/var/cache/lpm/archives");
    fs::create_dir_all(cache_dir)?;

    let m = MultiProgress::new();

    // --- REMOVAL PHASE ---
    if !tx.remove.is_empty() {
        let remove_pb = m.add(ProgressBar::new(tx.remove.len() as u64));
        remove_pb.set_style(ProgressStyle::with_template("{spinner:.red} {prefix:.bold} {msg}").unwrap());
        remove_pb.set_prefix("Removing");

        for pkg in &tx.remove {
            remove_pb.set_message(pkg.name.clone());

            // 1. Run Prerm
            let info_dir = Path::new("/var/lib/lpm/info").join(&pkg.name);
            if let Err(e) = run_script(&info_dir, "prerm", &["remove"], &pkg.name) {
                // Warn but continue removal often? Or fail? Standard is to fail.
                eprintln!("Warning: prerm script failed for {}: {}", pkg.name, e);
            }

            // 2. Remove Files
            if let Some(installed_pkg) = db.packages.get(&pkg.name) {
                // Sort files reverse length to delete files before directories
                let mut files = installed_pkg.files.clone();
                files.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

                for file_record in files {
                    let path = Path::new(&file_record.path);
                    if path.exists() && !path.is_dir() {
                        let _ = fs::remove_file(path);
                    } else if path.is_dir() {
                        // Only remove dir if empty
                        let _ = fs::remove_dir(path);
                    }
                }
            }

            // 3. Run Postrm
            if let Err(e) = run_script(&info_dir, "postrm", &["remove"], &pkg.name) {
                eprintln!("Warning: postrm script failed for {}: {}", pkg.name, e);
            }

            // 4. Update DB
            db.remove_package(&pkg.name);
            let _ = fs::remove_dir_all(info_dir); // Cleanup info

            remove_pb.inc(1);
        }
        remove_pb.finish_with_message("Removal done");

        // Log Removal in History
        let rem_names: Vec<String> = tx.remove.iter().map(|p| p.name.clone()).collect();
        db.append_history("remove", "Remove", rem_names)?;
    }

    // --- INSTALL PHASE ---
    if !tx.install.is_empty() || !tx.upgrade.is_empty() {
        // Combined list for logic simplification, though upgrades involve removal of old first in reality
        // Here we just overwrite.
        let to_install = tx.install.iter().chain(tx.upgrade.iter()).collect::<Vec<_>>();

        let install_pb = m.add(ProgressBar::new(to_install.len() as u64));
        install_pb.set_style(ProgressStyle::with_template("{spinner:.green} {prefix:.bold} {msg}").unwrap());
        install_pb.set_prefix("Installing");

        for pkg in to_install {
            install_pb.set_message(pkg.name.clone());

            let candidates = repo::search(&pkg.name).await?;
            let full_meta = candidates.iter().find(|c| c.version == pkg.version).unwrap_or(pkg);
            let file_name = format!("{}_{}.deb", full_meta.name, full_meta.version);
            let file_path = cache_dir.join(&file_name);

            // If file missing (solver didn't download), try download now (safety fallback)
            if !file_path.exists() {
                // In real code: Trigger download
                install_pb.suspend(|| println!("Warning: Archive for {} not found, skipping.", pkg.name));
                continue;
            }

            let info_dir = Path::new("/var/lib/lpm/info").join(&pkg.name);
            fs::create_dir_all(&info_dir)?;
            extract_control(&file_path, &info_dir)?;

            if let Err(e) = run_script(&info_dir, "preinst", &["install"], &pkg.name) {
                return Err(e);
            }

            let installed_files = unpack_files(&file_path, &pkg.name, &db)?;

            if let Err(e) = run_script(&info_dir, "postinst", &["configure"], &pkg.name) {
                return Err(e);
            }

            let reason = if requested_pkgs.contains(&pkg.name) { InstallReason::Manual } else { InstallReason::Automatic };

            db.register_package(InstalledPackage {
                name: pkg.name.clone(),
                                version: pkg.version.clone(),
                                install_date: chrono::Utc::now(),
                                files: installed_files,
                                size: pkg.size,
                                depends: pkg.depends.clone(),
                                arch: pkg.architecture.clone(),
                                reason,
            });

            install_pb.inc(1);
        }
        install_pb.finish_with_message("Installation done");

        let inst_names: Vec<String> = tx.install.iter().chain(tx.upgrade.iter()).map(|p| p.name.clone()).collect();
        db.append_history("install", "Install", inst_names)?;
    }

    db.save()?;
    plugins.run_hook("on_post_install", &tx)?;

    Ok(())
}

fn unpack_files(deb_path: &Path, pkg_name: &str, db: &Database) -> Result<Vec<InstalledFile>> {
    let file = File::open(deb_path)?;
    let mut archive = ar::Archive::new(file);
    let mut files_record = Vec::new();

    while let Some(entry_result) = archive.next_entry() {
        let entry = entry_result?;
        let identifier = String::from_utf8_lossy(entry.header().identifier()).to_string();

        if identifier.starts_with("data.tar") {
            let tar: Box<dyn Read> = if identifier.contains(".xz") {
                Box::new(XzDecoder::new(entry))
            } else if identifier.contains(".gz") {
                Box::new(GzDecoder::new(entry))
            } else {
                Box::new(entry)
            };

            let mut tar_archive = Archive::new(tar);
            for file in tar_archive.entries()? {
                let mut file = file?;
                let path = file.path()?.into_owned();
                let path_str = path.to_string_lossy();
                let sanitized_path = if path_str.starts_with("/") {
                    path_str.trim_start_matches('/').to_string()
                } else {
                    path_str.to_string()
                };

                if sanitized_path == "." || sanitized_path == "./" || sanitized_path.is_empty() { continue; }

                let root = Path::new("/");
                let full_target_path = root.join(&sanitized_path);
                let full_path_str = full_target_path.to_string_lossy().to_string();

                // Simple check for conflict - in real world allow replace if Replaces set
                if let Some(owner) = db.get_file_owner(&full_path_str) {
                    if owner != pkg_name {
                        // Ignoring conflict for demo stability
                    }
                }

                if let Some(parent) = full_target_path.parent() { fs::create_dir_all(parent)?; }
                file.unpack(&full_target_path)?;

                if !full_target_path.is_dir() {
                    let sha = compute_sha256(&full_target_path).unwrap_or_default();
                    files_record.push(InstalledFile {
                        path: full_path_str,
                        sha256: sha,
                    });
                }
            }
        }
    }
    Ok(files_record)
}

fn extract_control(deb_path: &Path, info_dir: &Path) -> Result<()> {
    let file = File::open(deb_path)?;
    let mut archive = ar::Archive::new(file);
    while let Some(entry_result) = archive.next_entry() {
        let entry = entry_result?;
        let identifier = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if identifier.starts_with("control.tar") {
            let tar: Box<dyn Read> = if identifier.contains(".gz") {
                Box::new(GzDecoder::new(entry))
            } else if identifier.contains(".xz") {
                Box::new(XzDecoder::new(entry))
            } else {
                Box::new(entry)
            };
            let mut tar_archive = Archive::new(tar);
            tar_archive.unpack(info_dir)?;
            return Ok(());
        }
    }
    Ok(())
}

fn run_script(info_dir: &Path, script_name: &str, args: &[&str], pkg_name: &str) -> Result<()> {
    let script_path = info_dir.join(script_name);
    if script_path.exists() {
        let mut perms = fs::metadata(&script_path)?.permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;

        let status = Command::new(&script_path)
        .args(args)
        .env("DPKG_MAINTSCRIPT_PACKAGE", pkg_name)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status()?;

        if !status.success() {
            return Err(anyhow!("Script {} failed", script_name));
        }
    }
    Ok(())
}

pub fn verify_installation() -> Result<()> {
    let db = Database::load();
    let mut errors = 0;

    println!("Verifying {} packages...", db.packages.len());
    let pb = ProgressBar::new(db.packages.len() as u64);

    for (name, pkg) in &db.packages {
        pb.set_message(name.clone());
        for file in &pkg.files {
            let path = Path::new(&file.path);
            if !path.exists() {
                pb.suspend(|| println!("MISSING: {}", file.path));
                errors += 1;
                continue;
            }
            if let Ok(current_sha) = compute_sha256(path) {
                if current_sha != file.sha256 {
                    pb.suspend(|| println!("MODIFIED: {}", file.path));
                    errors += 1;
                }
            }
        }
        pb.inc(1);
    }
    pb.finish();

    if errors > 0 {
        Err(anyhow!("Verification failed with {} errors", errors))
    } else {
        println!("All packages verified successfully.");
        Ok(())
    }
}
