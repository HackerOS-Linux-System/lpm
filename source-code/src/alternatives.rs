//! Minimal update-alternatives implementation.
//!
//! When a .deb is installed, its postinst script typically calls:
//!   update-alternatives --install /usr/bin/vim vim /usr/bin/vim.basic 30
//!
//! This creates:
//!   /etc/alternatives/vim -> /usr/bin/vim.basic   (managed symlink)
//!   /usr/bin/vim          -> /etc/alternatives/vim (generic name)
//!
//! We implement this by:
//!   1. After extracting a package, scanning for broken symlinks
//!   2. For each broken symlink pointing to /etc/alternatives/X,
//!      finding a real binary that could serve as the alternative
//!      and creating /etc/alternatives/X -> actual_binary

use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};

/// After installing all files from a package, fix any broken
/// /etc/alternatives/* symlinks that the postinst would have created.
pub fn fix_alternatives(installed_files: &[PathBuf]) {
    for path in installed_files {
        // Only care about symlinks
        let meta = match path.symlink_metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.file_type().is_symlink() {
            continue;
        }

        // Read where the symlink points
        let target = match std::fs::read_link(path) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let target_str = target.to_string_lossy();

        // Only handle /etc/alternatives/* targets
        if !target_str.starts_with("/etc/alternatives/") {
            continue;
        }

        let alt_path = Path::new(target_str.as_ref());

        // If /etc/alternatives/X already exists and points somewhere valid — skip
        if alt_path.exists() {
            continue;
        }

        // Try to find a real binary to use as the alternative.
        // Convention: alternatives name == binary name, look for:
        //   {dirname}/{name}.basic
        //   {dirname}/{name}.tiny
        //   {dirname}/{name}-{version}
        //   {dirname}/x{name}    (e.g. xxd for vim)
        let alt_name = match alt_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_owned(),
            None => continue,
        };

        // The generic name path (e.g. /usr/bin/vim)
        let generic_dir = match path.parent() {
            Some(d) => d.to_owned(),
            None => continue,
        };

        let candidates = [
            generic_dir.join(format!("{}.basic",  &alt_name)),
            generic_dir.join(format!("{}.tiny",   &alt_name)),
            generic_dir.join(format!("{}.nox",    &alt_name)),
            generic_dir.join(format!("{}.gtk",    &alt_name)),
            generic_dir.join(format!("{}.gnome",  &alt_name)),
            generic_dir.join(format!("{}editor",  &alt_name)),
            // Also check if there's an exact match in a sibling location
            PathBuf::from(format!("/usr/lib/{}/{}", &alt_name, &alt_name)),
        ];

        let found = candidates.iter().find(|c| c.exists());

        if let Some(real_bin) = found {
            // Create /etc/alternatives/vim -> /usr/bin/vim.basic
            if let Some(parent) = alt_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::remove_file(alt_path);
            let _ = unix_fs::symlink(real_bin, alt_path);
        } else {
            // Last resort: if we can find the broken symlink's target binary
            // anywhere in the extracted files list, use that
            let fallback = installed_files.iter().find(|f| {
                f.file_name().and_then(|n| n.to_str())
                    .map_or(false, |n| {
                        n == format!("{}.basic", alt_name)
                            || n == format!("{}.tiny", alt_name)
                            || (n != alt_name && n.starts_with(&alt_name))
                    })
            });

            if let Some(real_bin) = fallback {
                if let Some(parent) = alt_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::remove_file(alt_path);
                let _ = unix_fs::symlink(real_bin, alt_path);
            }
        }
    }
}

/// Run ldconfig to update shared library cache.
/// Called after installing packages that provide .so files.
pub fn run_ldconfig() {
    let _ = std::process::Command::new("ldconfig")
        .status();
}

/// Check if any installed file is a shared library that needs ldconfig.
pub fn needs_ldconfig(files: &[PathBuf]) -> bool {
    files.iter().any(|f| {
        let s = f.to_string_lossy();
        (s.contains("/lib/") || s.contains("/lib64/"))
            && (s.contains(".so") )
    })
}
