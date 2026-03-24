use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as FmtWrite;

#[cfg(feature = "sat-solver")]
use varisat::{CnfFormula, ExtendFormula, Lit, Var, Solver as VarisatSolver};

use crate::cache::PackageCache;
use crate::db::InstalledDb;
use crate::package::{parse_dep_field, version_cmp, Package};
use crate::solver::TransactionPlan;   // <-- KLUCZOWY IMPORT

pub struct SatSolver<'a> {
    cache: &'a PackageCache,
    db: &'a InstalledDb,
    all_pkgs: Vec<&'a Package>,
    pkg_to_var: HashMap<String, Var>,
    var_to_pkg: Vec<String>,
    formula: CnfFormula,
        user_requests: HashSet<String>,
        costs: HashMap<Var, u64>,
}

#[cfg(feature = "sat-solver")]
impl<'a> SatSolver<'a> {
    pub fn new(cache: &'a PackageCache, db: &'a InstalledDb) -> Self {
        SatSolver {
            cache,
            db,
            all_pkgs: cache.all_packages().into_iter().collect(),
            pkg_to_var: HashMap::new(),
            var_to_pkg: Vec::new(),
            formula: CnfFormula::new(),
                user_requests: HashSet::new(),
                costs: HashMap::new(),
        }
    }

    pub fn solve(
        mut self,
        names: &[String],
        no_recommends: bool,
    ) -> Result<TransactionPlan> {
        // Sprawdź, czy cache nie jest pusty
        if self.all_pkgs.is_empty() {
            bail!("Package cache is empty. Run `lpm update` first.");
        }

        self.map_packages();

        for name in names {
            let name = name.split(':').next().unwrap_or(name).to_owned();
            if let Some(&var) = self.pkg_to_var.get(&name) {
                self.user_requests.insert(name.clone());
                self.formula.add_clause(&[Lit::positive(var)]);
            } else {
                bail!(
                    "Package '{}' not found in cache.\n  Hint: run `lpm update` and check if the package exists in your repositories.",
                    name
                );
            }
        }

        self.add_dependencies(no_recommends);
        self.add_conflicts_and_breaks();
        self.add_installed_packages();

        let mut sat_solver = VarisatSolver::new();
        sat_solver.add_formula(&self.formula);

        if !sat_solver.solve().map_err(|e| anyhow::anyhow!("SAT solver error: {}", e))? {
            let explanation = self.explain_unsatisfiable(&sat_solver);
            bail!("Cannot resolve dependencies:\n{}", explanation);
        }

        let model = sat_solver
        .model()
        .context("No model found after successful solve")?;

        self.build_plan(&model)
    }

    fn map_packages(&mut self) {
        for (idx, pkg) in self.all_pkgs.iter().enumerate() {
            let var = Var::from_index(idx);
            self.pkg_to_var.insert(pkg.name.clone(), var);
            self.var_to_pkg.push(pkg.name.clone());
            self.costs.insert(var, 1);
        }
    }

    fn add_dependencies(&mut self, no_recommends: bool) {
        for pkg in &self.all_pkgs {
            let pkg_var = self.pkg_to_var[&pkg.name];

            let deps = vec![
                pkg.depends.as_deref(),
                pkg.pre_depends.as_deref(),
                if no_recommends { None } else { pkg.recommends.as_deref() },
            ];

            for field in deps.iter().flatten() {
                let groups = parse_dep_field(field);
                if groups.is_empty() {
                    continue;
                }

                for group in groups {
                    let mut clause = vec![Lit::negative(pkg_var)];

                    for alt in &group.alternatives {
                        if let Some(&dep_var) = self.pkg_to_var.get(&alt.name) {
                            clause.push(Lit::positive(dep_var));
                        } else if let Some(provided_by) = self.find_provider(&alt.name) {
                            for prov in provided_by {
                                clause.push(Lit::positive(prov));
                            }
                        }
                    }

                    if clause.len() > 1 {
                        self.formula.add_clause(&clause);
                    }
                }
            }
        }
    }

    fn find_provider(&self, provides_name: &str) -> Option<Vec<Var>> {
        let mut providers = Vec::new();
        for pkg in &self.all_pkgs {
            if let Some(prov) = &pkg.provides {
                if prov.contains(provides_name) {
                    providers.push(self.pkg_to_var[&pkg.name]);
                }
            }
        }
        if providers.is_empty() {
            None
        } else {
            Some(providers)
        }
    }

    fn add_conflicts_and_breaks(&mut self) {
        for pkg in &self.all_pkgs {
            let pkg_var = self.pkg_to_var[&pkg.name];

            if let Some(conflicts) = &pkg.conflicts {
                for group in parse_dep_field(conflicts) {
                    for alt in &group.alternatives {
                        if let Some(&alt_var) = self.pkg_to_var.get(&alt.name) {
                            self.formula.add_clause(&[Lit::negative(pkg_var), Lit::negative(alt_var)]);
                        }
                    }
                }
            }

            if let Some(breaks) = &pkg.breaks {
                for group in parse_dep_field(breaks) {
                    for alt in &group.alternatives {
                        if let Some(&alt_var) = self.pkg_to_var.get(&alt.name) {
                            self.formula.add_clause(&[Lit::negative(pkg_var), Lit::negative(alt_var)]);
                        }
                    }
                }
            }
        }
    }

    fn add_installed_packages(&mut self) {
        for inst in self.db.list_all().unwrap_or_default() {
            if let Some(&var) = self.pkg_to_var.get(&inst.name) {
                self.formula.add_clause(&[Lit::positive(var)]);
            }
        }
    }

    fn build_plan(&self, model: &[Lit]) -> Result<TransactionPlan> {
        let mut to_install = Vec::new();
        let mut to_upgrade = Vec::new();
        let mut upgrade_from = HashMap::new();

        for (idx, pkg) in self.all_pkgs.iter().enumerate() {
            let var = Var::from_index(idx);
            let is_true = model.contains(&Lit::positive(var));
            if is_true {
                if let Some(inst) = self.db.get(&pkg.name) {
                    if version_cmp(&pkg.version, &inst.version) == std::cmp::Ordering::Greater {
                        upgrade_from.insert(pkg.name.clone(), inst.version.clone());
                        to_upgrade.push((*pkg).clone());
                    }
                } else {
                    to_install.push((*pkg).clone());
                }
            }
        }

        let download_bytes = to_install.iter().chain(&to_upgrade)
        .map(|p| p.download_size.unwrap_or(0)).sum();
        let install_bytes = to_install.iter().chain(&to_upgrade)
        .map(|p| p.installed_size_kb.unwrap_or(0) * 1024).sum();

        Ok(TransactionPlan {
            to_install,
            to_upgrade,
            to_remove: Vec::new(),
           to_autoremove: Vec::new(),
           upgrade_from,
           download_bytes,
           install_bytes,
           freed_bytes: 0,
           warnings: Vec::new(),
        })
    }

    fn explain_unsatisfiable(&self, _solver: &VarisatSolver) -> String {
        let mut explanation = String::new();
        writeln!(&mut explanation, "Unable to resolve dependencies.").unwrap();
        writeln!(&mut explanation).unwrap();

        writeln!(&mut explanation, "Possible reasons:").unwrap();
        for name in &self.user_requests {
            writeln!(&mut explanation, "  - Requested package '{}' may be unavailable or conflicting", name).unwrap();
        }
        explanation
    }
}

#[cfg(not(feature = "sat-solver"))]
pub fn resolve_with_sat(
    _cache: &PackageCache,
    _db: &InstalledDb,
    _names: &[String],
    _no_recommends: bool,
) -> Result<TransactionPlan> {
    anyhow::bail!("SAT solver not compiled in. Enable feature 'sat-solver' or use default resolver.");
}

#[cfg(feature = "sat-solver")]
pub fn resolve_with_sat(
    cache: &PackageCache,
    db: &InstalledDb,
    names: &[String],
    no_recommends: bool,
) -> Result<TransactionPlan> {
    let solver = SatSolver::new(cache, db);
    solver.solve(names, no_recommends)
}
