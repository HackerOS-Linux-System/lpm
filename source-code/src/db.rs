use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use anyhow::Result;
use chrono::{DateTime, Utc};

const DB_PATH: &str = "/var/lib/lpm/db.json";
const HISTORY_PATH: &str = "/var/lib/lpm/history.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum InstallReason {
    Manual,
    Automatic,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstalledFile {
    pub path: String,
    pub sha256: String, // Post-install checksum for verification
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub install_date: DateTime<Utc>,
    pub files: Vec<InstalledFile>,
    pub size: u64,
    pub depends: Vec<String>,
    #[serde(default)]
    pub arch: String, // Added to prevent solver crashes
    #[serde(default = "default_reason")]
    pub reason: InstallReason,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryRecord {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub command: String,
    pub action: String, // "Install", "Remove", "Update"
    pub packages: Vec<String>,
}

fn default_reason() -> InstallReason { InstallReason::Manual }

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Database {
    pub packages: HashMap<String, InstalledPackage>,
}

impl Database {
    pub fn load() -> Self {
        if Path::new(DB_PATH).exists() {
            let content = fs::read_to_string(DB_PATH).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Database::default()
        }
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = Path::new(DB_PATH).parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(DB_PATH, content)?;
        Ok(())
    }

    pub fn is_installed(&self, pkg_name: &str) -> bool {
        self.packages.contains_key(pkg_name)
    }

    pub fn register_package(&mut self, pkg: InstalledPackage) {
        self.packages.insert(pkg.name.clone(), pkg);
    }

    pub fn remove_package(&mut self, pkg_name: &str) -> Option<InstalledPackage> {
        self.packages.remove(pkg_name)
    }

    pub fn get_file_owner(&self, path: &str) -> Option<String> {
        for (pkg_name, data) in &self.packages {
            for file in &data.files {
                if file.path == path {
                    return Some(pkg_name.clone());
                }
            }
        }
        None
    }

    // Logic for autoremove: Find packages that are Automatic and not depended upon by any Manual package.
    pub fn get_orphans(&self) -> Vec<String> {
        let mut needed = HashSet::new();

        // 1. Mark all manual packages and their recursive dependencies as needed
        for pkg in self.packages.values() {
            if pkg.reason == InstallReason::Manual {
                self.mark_dependencies(&pkg.name, &mut needed);
            }
        }

        // 2. Identify packages not in 'needed' list
        let mut orphans = Vec::new();
        for (name, pkg) in &self.packages {
            if pkg.reason == InstallReason::Automatic && !needed.contains(name) {
                orphans.push(name.clone());
            }
        }
        orphans
    }

    fn mark_dependencies(&self, pkg_name: &str, marked: &mut HashSet<String>) {
        if marked.contains(pkg_name) { return; }
        marked.insert(pkg_name.to_string());

        if let Some(pkg) = self.packages.get(pkg_name) {
            for dep in &pkg.depends {
                // Simplistic dependency resolution (name match)
                // In reality, deps are "libfoo (>= 1.2)". Here we assume simple names for the demo.
                let clean_dep = dep.split_whitespace().next().unwrap_or(dep);
                self.mark_dependencies(clean_dep, marked);
            }
        }
    }

    pub fn append_history(&self, command: &str, action: &str, packages: Vec<String>) -> Result<()> {
        let record = HistoryRecord {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            command: command.to_string(),
            action: action.to_string(),
            packages,
        };

        let mut history: Vec<HistoryRecord> = if Path::new(HISTORY_PATH).exists() {
            let content = fs::read_to_string(HISTORY_PATH)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Vec::new()
        };

        history.push(record);

        if let Some(parent) = Path::new(HISTORY_PATH).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(HISTORY_PATH, serde_json::to_string_pretty(&history)?)?;
        Ok(())
    }

    pub fn get_history(&self) -> Result<Vec<HistoryRecord>> {
        if Path::new(HISTORY_PATH).exists() {
            let content = fs::read_to_string(HISTORY_PATH)?;
            Ok(serde_json::from_str(&content).unwrap_or_default())
        } else {
            Ok(Vec::new())
        }
    }
}
