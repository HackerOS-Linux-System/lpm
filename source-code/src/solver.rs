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
    pub to_install:   Vec<Package>,
    pub to_upgrade:   Vec<Package>,
    pub to_remove:    Vec<String>,   // package names
    pub to_autoremove: Vec<String>,

    /// For upgrades: old version string
    pub upgrade_from: HashMap<String, String>,

    pub download_bytes:  u64,
    pub install_bytes:   u64,
    pub freed_bytes:     u64,

    /// Warnings accumulated during solving
    pub warnings: Vec<String>,
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
    //  Install
    // ──────────────────────────────────────────────────────────

    pub fn resolve_install(
        &self,
        names:       &[String],
        no_recommends: bool,
    ) -> Result<TransactionPlan> {
        let mut plan   = TransactionPlan::default();
        let mut seen:   HashSet<String> = HashSet::new();
        let mut queue:  VecDeque<(String, bool)> = VecDeque::new(); // (name, is_dep)

        // Validate all requested packages exist first
        for name in names {
            if self.cache.get(name).is_none() && !self.db.is_installed(name) {
                bail!(
                    "No match for package: '{}'\n  Run `lpm update` to refresh the package index.",
                    name
                );
            }
            queue.push_back((name.clone(), false));
        }

        while let Some((name, is_dep)) = queue.pop_front() {
            if seen.contains(&name) { continue; }
            seen.insert(name.clone());

            // Already installed?
            if let Some(inst) = self.db.get(&name) {
                // Check if available version is newer
                if let Some(avail) = self.cache.get(&name) {
                    if version_cmp(&avail.version, &inst.version) == std::cmp::Ordering::Greater {
                        plan.upgrade_from.insert(name.clone(), inst.version.clone());
                        plan.download_bytes += avail.download_size.unwrap_or(0);
                        plan.install_bytes  += avail.installed_size_kb.unwrap_or(0) * 1024;
                        plan.to_upgrade.push(avail.clone());
                        // Also resolve dependencies of new version
                        self.enqueue_deps(avail, no_recommends, &mut queue);
                    }
                    // else: same or older – skip
                }
                continue;
            }

            let pkg = match self.cache.get(&name) {
                Some(p) => p.clone(),
                None    => {
                    plan.warnings.push(format!(
                        "Cannot find '{}' in package index (missing dependency, skipped)", name
                    ));
                    continue;
                }
            };

            plan.download_bytes += pkg.download_size.unwrap_or(0);
            plan.install_bytes  += pkg.installed_size_kb.unwrap_or(0) * 1024;

            self.enqueue_deps(&pkg, no_recommends, &mut queue);
            plan.to_install.push(pkg);
        }

        // Sort so dependencies come before dependents (simple: alphabetical is fine
        // since dpkg will handle the real order; for us extraction order matters less)
        plan.to_install.sort_by(|a, b| a.name.cmp(&b.name));
        plan.to_upgrade.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(plan)
    }

    fn enqueue_deps(
        &self,
        pkg:        &Package,
        no_recommends: bool,
        queue:      &mut VecDeque<(String, bool)>,
    ) {
        let dep_fields = [
            pkg.pre_depends.as_deref(),
            pkg.depends.as_deref(),
            if no_recommends { None } else { pkg.recommends.as_deref() },
        ];

        for field in dep_fields.iter().flatten() {
            for group in parse_dep_field(field) {
                // Pick the first satisfiable alternative
                let chosen = group.alternatives.iter().find(|alt| {
                    // Prefer already installed
                    if self.db.is_installed(&alt.name) {
                        if let Some(c) = &alt.constraint {
                            if let Some(inst) = self.db.get(&alt.name) {
                                return version_satisfies(&inst.version, &c.op, &c.version);
                            }
                        }
                        return true;
                    }
                    self.cache.get(&alt.name).is_some()
                });

                if let Some(dep) = chosen {
                    queue.push_back((dep.name.clone(), true));
                }
                // else: unresolvable – will warn during install
            }
        }
    }

    // ──────────────────────────────────────────────────────────
    //  Remove
    // ──────────────────────────────────────────────────────────

    pub fn resolve_remove(&self, names: &[String]) -> Result<TransactionPlan> {
        let mut plan = TransactionPlan::default();

        for name in names {
            if let Some(inst) = self.db.get(name) {
                plan.freed_bytes += inst.installed_size_kb * 1024;
                plan.to_remove.push(name.clone());
            } else {
                bail!(
                    "Package '{}' is not installed.",
                    name
                );
            }
        }

        Ok(plan)
    }

    // ──────────────────────────────────────────────────────────
    //  Upgrade (all)
    // ──────────────────────────────────────────────────────────

    pub fn resolve_upgrade(&self) -> Result<TransactionPlan> {
        let mut plan = TransactionPlan::default();

        let installed = self.db.list_all()?;
        for inst in installed {
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
    //  Autoremove
    // ──────────────────────────────────────────────────────────

    pub fn resolve_autoremove(&self) -> Result<TransactionPlan> {
        let mut plan = TransactionPlan::default();

        // Collect all packages needed by user-installed packages
        let user_pkgs = self.db.list_user_installed()?;
        let mut needed: HashSet<String> = user_pkgs.iter().map(|p| p.name.clone()).collect();

        // Expand to all transitive deps of user packages
        let mut queue: VecDeque<String> = needed.iter().cloned().collect();
        while let Some(name) = queue.pop_front() {
            if let Some(pkg) = self.db.get(&name) {
                if let Some(ref deps) = pkg.depends {
                    for group in parse_dep_field(deps) {
                        // Take first installed alternative
                        let chosen = group.alternatives.iter().find(|a| {
                            self.db.is_installed(&a.name)
                        });
                        if let Some(dep) = chosen {
                            if needed.insert(dep.name.clone()) {
                                queue.push_back(dep.name.clone());
                            }
                        }
                    }
                }
            }
        }

        // Packages installed as deps but no longer needed
        let all_installed = self.db.list_all()?;
        for pkg in all_installed {
            if !needed.contains(&pkg.name) {
                plan.freed_bytes += pkg.installed_size_kb * 1024;
                plan.to_autoremove.push(pkg.name.clone());
            }
        }

        plan.to_autoremove.sort();
        Ok(plan)
    }
}
