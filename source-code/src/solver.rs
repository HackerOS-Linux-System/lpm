use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::cache::PackageCache;
use crate::db::InstalledDb;
use crate::package::{parse_dep_field, version_cmp, version_satisfies, Package};

// ─────────────────────────────────────────────────────────────
//  TransactionPlan
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct TransactionPlan {
    pub to_install:    Vec<Package>,
    pub to_upgrade:    Vec<Package>,
    pub to_remove:     Vec<String>,
    pub to_autoremove: Vec<String>,
    pub upgrade_from:  HashMap<String, String>,
    pub download_bytes: u64,
    pub install_bytes:  u64,
    pub freed_bytes:    u64,
    pub warnings:       Vec<String>,
}

impl TransactionPlan {
    pub fn is_empty(&self) -> bool {
        self.to_install.is_empty()
        && self.to_upgrade.is_empty()
        && self.to_remove.is_empty()
        && self.to_autoremove.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────
//  Solver
// ─────────────────────────────────────────────────────────────

pub struct Solver<'a> {
    cache: &'a PackageCache,
    db:    &'a InstalledDb,
}

impl<'a> Solver<'a> {
    pub fn new(cache: &'a PackageCache, db: &'a InstalledDb) -> Self {
        Solver { cache, db }
    }

    // ──────────────────────────────────────────────────────────
    //  resolve_install
    //
    //  Recommends are OFF by default (no_recommends = true by default
    //  in cli.rs). User must pass --with-recommends to enable them.
    //  This matches DNF/zypper behaviour and avoids surprise bloat.
    // ──────────────────────────────────────────────────────────

    pub fn resolve_install(
        &self,
        names:          &[String],
        no_recommends:  bool,
    ) -> Result<TransactionPlan> {
        let mut plan  = TransactionPlan::default();
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, bool)> = VecDeque::new();

        for name in names {
            // Strip arch qualifier if present (e.g. "vim:amd64" → "vim")
            let name = name.split(':').next().unwrap_or(name).to_owned();
            if self.cache.get(&name).is_none() {
                bail!(
                    "No match for package: '{}'\n  Hint: run `lpm update` to refresh the package index.",
                    name
                );
            }
            queue.push_back((name, true));
        }

        while let Some((name, explicit)) = queue.pop_front() {
            if seen.contains(&name) { continue; }
            seen.insert(name.clone());

            let avail = match self.cache.get(&name) {
                Some(p) => p.clone(),
                None => {
                    plan.warnings.push(format!(
                        "dependency '{}' not found in package index — skipped", name
                    ));
                    continue;
                }
            };

            // Skip packages marked as Priority: required/important/standard
            // unless explicitly requested — they're already on the system
            let priority = avail.priority.as_deref().unwrap_or("");
            if !explicit && matches!(priority, "required" | "important" | "standard") {
                // Still enqueue their deps if they're not installed
                if !self.db.is_installed(&name) {
                    self.enqueue_deps(&avail, true, &mut queue); // always no-recommends for system pkgs
                }
                continue;
            }

            if let Some(inst) = self.db.get(&name) {
                if explicit {
                    match version_cmp(&avail.version, &inst.version) {
                        std::cmp::Ordering::Greater => {
                            plan.upgrade_from.insert(name.clone(), inst.version.clone());
                            plan.download_bytes += avail.download_size.unwrap_or(0);
                            plan.install_bytes  += avail.installed_size_kb.unwrap_or(0) * 1024;
                            self.enqueue_deps(&avail, no_recommends, &mut queue);
                            plan.to_upgrade.push(avail);
                        }
                        _ => {
                            if !package_physically_present(&inst) {
                                plan.download_bytes += avail.download_size.unwrap_or(0);
                                plan.install_bytes  += avail.installed_size_kb.unwrap_or(0) * 1024;
                                self.enqueue_deps(&avail, no_recommends, &mut queue);
                                plan.to_install.push(avail);
                            }
                            // else: up to date, skip
                        }
                    }
                }
                // dep already installed → satisfied
                continue;
            }

            // Not in DB → install
            plan.download_bytes += avail.download_size.unwrap_or(0);
            plan.install_bytes  += avail.installed_size_kb.unwrap_or(0) * 1024;
            self.enqueue_deps(&avail, no_recommends, &mut queue);
            plan.to_install.push(avail);
        }

        plan.to_install.sort_by(|a, b| a.name.cmp(&b.name));
        plan.to_upgrade.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(plan)
    }

    fn enqueue_deps(
        &self,
        pkg:           &Package,
        no_recommends: bool,
        queue:         &mut VecDeque<(String, bool)>,
    ) {
        // Always follow: Pre-Depends, Depends
        // Only follow Recommends if --with-recommends is passed
        // Never follow: Suggests
        let fields: &[Option<&str>] = &[
            pkg.pre_depends.as_deref(),
            pkg.depends.as_deref(),
            if no_recommends { None } else { pkg.recommends.as_deref() },
        ];

        for field in fields.iter().flatten() {
            for group in parse_dep_field(field) {
                // Prefer an already-installed alternative
                let chosen = group.alternatives.iter().find(|alt| {
                    if let Some(inst) = self.db.get(&alt.name) {
                        if let Some(ref c) = alt.constraint {
                            return version_satisfies(&inst.version, &c.op, &c.version);
                        }
                        return true;
                    }
                    false
                })
                // Otherwise prefer first alternative available in cache
                .or_else(|| {
                    group.alternatives.iter().find(|alt| {
                        self.cache.get(&alt.name).is_some()
                    })
                });

                if let Some(dep) = chosen {
                    // Strip arch qualifier
                    let dep_name = dep.name.split(':').next().unwrap_or(&dep.name).to_owned();
                    queue.push_back((dep_name, false));
                }
            }
        }
    }

    // ──────────────────────────────────────────────────────────
    //  resolve_remove
    // ──────────────────────────────────────────────────────────

    pub fn resolve_remove(&self, names: &[String]) -> Result<TransactionPlan> {
        let mut plan = TransactionPlan::default();
        for name in names {
            match self.db.get(name) {
                Some(inst) => {
                    plan.freed_bytes += inst.installed_size_kb * 1024;
                    plan.to_remove.push(name.clone());
                }
                None => bail!("Package '{}' is not installed.", name),
            }
        }
        Ok(plan)
    }

    // ──────────────────────────────────────────────────────────
    //  resolve_upgrade
    // ──────────────────────────────────────────────────────────

    pub fn resolve_upgrade(&self) -> Result<TransactionPlan> {
        let mut plan = TransactionPlan::default();
        for inst in self.db.list_all()? {
            if let Some(avail) = self.cache.get(&inst.name) {
                if version_cmp(&avail.version, &inst.version) == std::cmp::Ordering::Greater {
                    plan.upgrade_from.insert(inst.name.clone(), inst.version.clone());
                    plan.download_bytes += avail.download_size.unwrap_or(0);
                    plan.install_bytes  += avail.installed_size_kb.unwrap_or(0) * 1024;
                    plan.to_upgrade.push(avail.clone());
                }
            }
        }
        plan.to_upgrade.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(plan)
    }

    // ──────────────────────────────────────────────────────────
    //  resolve_autoremove
    // ──────────────────────────────────────────────────────────

    pub fn resolve_autoremove(&self) -> Result<TransactionPlan> {
        let mut plan = TransactionPlan::default();

        let user_pkgs = self.db.list_user_installed()?;
        let mut needed: HashSet<String> = user_pkgs.iter().map(|p| p.name.clone()).collect();

        let mut queue: VecDeque<String> = needed.iter().cloned().collect();
        while let Some(name) = queue.pop_front() {
            if let Some(pkg) = self.db.get(&name) {
                if let Some(ref dep_str) = pkg.depends {
                    for group in parse_dep_field(dep_str) {
                        if let Some(dep) = group.alternatives.iter()
                            .find(|a| self.db.is_installed(&a.name))
                            {
                                if needed.insert(dep.name.clone()) {
                                    queue.push_back(dep.name.clone());
                                }
                            }
                    }
                }
            }
        }

        for pkg in self.db.list_all()? {
            if !needed.contains(&pkg.name) {
                plan.freed_bytes += pkg.installed_size_kb * 1024;
                plan.to_autoremove.push(pkg.name.clone());
            }
        }

        plan.to_autoremove.sort();
        Ok(plan)
    }
}

// ─────────────────────────────────────────────────────────────
//  Physical presence check
// ─────────────────────────────────────────────────────────────

fn package_physically_present(inst: &crate::db::InstalledPackage) -> bool {
    match inst.files.split(';').find(|s| !s.is_empty()) {
        None    => true, // no file list → assume present
        Some(f) => std::path::Path::new(f).exists(),
    }
}
