use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

const KEYRING_DIR: &str = "/etc/apt/trusted.gpg.d";

pub struct Keyring;

impl Keyring {
    /// Dodaje klucz GPG z pliku lub URL.
    pub fn add(path: &str) -> Result<()> {
        let dest_dir = Path::new(KEYRING_DIR);
        fs::create_dir_all(dest_dir).context("Cannot create keyring dir")?;

        let src = Path::new(path);
        if src.exists() {
            let name = src.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
            let dest = dest_dir.join(format!("lpm_{}", name));
            fs::copy(src, &dest)?;
            println!("Key added: {}", dest.display());
            Ok(())
        } else if path.starts_with("http://") || path.starts_with("https://") {
            let tmp = tempfile::NamedTempFile::new()?;
            let status = Command::new("curl")
            .arg("-fsSL")
            .arg(path)
            .arg("-o")
            .arg(tmp.path())
            .status()?;
            if !status.success() {
                bail!("Failed to download key from {}", path);
            }
            let name = Path::new(path).file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
            let dest = dest_dir.join(format!("lpm_{}", name));
            fs::copy(tmp.path(), &dest)?;
            println!("Key downloaded and added: {}", dest.display());
            Ok(())
        } else {
            bail!("Key file not found: {}", path);
        }
    }

    /// Wyświetla listę kluczy w keyringu.
    pub fn list() -> Result<Vec<String>> {
        let dir = Path::new(KEYRING_DIR);
        if !dir.exists() {
            return Ok(vec![]);
        }

        let mut keys = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "gpg") {
                keys.push(path.file_name().unwrap().to_string_lossy().to_string());
            }
        }
        Ok(keys)
    }

    /// Usuwa klucz o podanej nazwie.
    pub fn remove(name: &str) -> Result<()> {
        let dest = Path::new(KEYRING_DIR).join(name);
        if dest.exists() {
            fs::remove_file(&dest)?;
            println!("Removed key: {}", dest.display());
            Ok(())
        } else {
            bail!("Key {} not found in keyring", name);
        }
    }
}
