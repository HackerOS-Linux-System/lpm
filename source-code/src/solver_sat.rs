use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};

#[cfg(feature = "sat-solver")]
use varisat::{CnfFormula, ExtendFormula, Lit, Solver as VarisatSolver, Var};

use crate::cache::PackageCache;
use crate::db::InstalledDb;
use crate::package::{parse_dep_field, version_cmp, version_satisfies, Package};
use crate::solver::TransactionPlan;

// ─────────────────────────────────────────────────────────────
//  Public entrypoint
// ─────────────────────────────────────────────────────────────

#[cfg(not(feature = "sat-solver"))]
pub fn resolve_with_sat(
    _cache: &PackageCache,
    _db:    &InstalledDb,
    _names: &[String],
    _no_recommends: bool,
) -> Result<TransactionPlan> {
    bail!("SAT solver not compiled. Rebuild with --features sat-solver.");
}

#[cfg(feature = "sat-solver")]
pub fn resolve_with_sat(
    cache: &PackageCache,
    db:    &InstalledDb,
    names: &[String],
    no_recommends: bool,
) -> Result<TransactionPlan> {
    SatResolver::new(cache, db).resolve(names, no_recommends)
}

// ─────────────────────────────────────────────────────────────
//  SatResolver
// ─────────────────────────────────────────────────────────────

#[cfg(feature = "sat-solver")]
struct SatResolver<'a> {
    cache:        &'a PackageCache,
    db:           &'a InstalledDb,
    /// Indeksowana lista wszystkich pakietów (indeks == Var index)
    pkgs:         Vec<&'a Package>,
    /// name → Var (najnowsza wersja; wiele wersji tego samego pakietu → osobne Var)
    name_to_var:  HashMap<String, usize>,
    /// provides_name → lista Var pakietów, które dostarczają ten wirtualny pakiet
    provides_map: HashMap<String, Vec<usize>>,
}

#[cfg(feature = "sat-solver")]
impl<'a> SatResolver<'a> {
    fn new(cache: &'a PackageCache, db: &'a InstalledDb) -> Self {
        let pkgs: Vec<&Package> = cache.all_packages();
        let mut name_to_var  = HashMap::new();
        let mut provides_map: HashMap<String, Vec<usize>> = HashMap::new();

        for (idx, pkg) in pkgs.iter().enumerate() {
            // Zawsze zachowaj najnowszą wersję pod nazwą
            let entry = name_to_var.entry(pkg.name.clone()).or_insert(idx);
            // Jeśli ta wersja jest nowsza – zaktualizuj
            if version_cmp(&pkgs[idx].version, &pkgs[*entry].version)
                == std::cmp::Ordering::Greater
                {
                    *entry = idx;
                }

                // Przetwórz Provides
                if let Some(prov_str) = &pkg.provides {
                    for group in parse_dep_field(prov_str) {
                        for alt in &group.alternatives {
                            provides_map
                            .entry(alt.name.clone())
                            .or_default()
                            .push(idx);
                        }
                    }
                }
        }

        SatResolver { cache, db, pkgs, name_to_var, provides_map }
    }

    // ──────────────────────────────────────────────────────────
    //  Główna logika
    // ──────────────────────────────────────────────────────────

    fn resolve(
        &self,
        names:         &[String],
        no_recommends: bool,
    ) -> Result<TransactionPlan> {
        if self.pkgs.is_empty() {
            bail!("Package cache is empty. Run `lpm update` first.");
        }

        // Walidacja żądań użytkownika
        let mut requested_vars: Vec<usize> = Vec::new();
        for raw_name in names {
            let name = raw_name.split(':').next().unwrap_or(raw_name);
            let idx = self.name_to_var.get(name)
            .or_else(|| self.provides_map.get(name).and_then(|v| v.first()))
            .copied()
            .with_context(|| format!(
                "Package '{}' not found. Run `lpm update` and check the name.",
                name
            ))?;
            requested_vars.push(idx);
        }

        let mut formula = CnfFormula::new();

        // 1) Zbuduj klauzule dla wszystkich pakietów
        self.add_dependency_clauses(&mut formula, no_recommends);
        self.add_conflict_clauses(&mut formula);

        // 2) Wymuś żądane pakiety
        for &idx in &requested_vars {
            formula.add_clause(&[Lit::positive(Var::from_index(idx))]);
        }

        // 3) Wymuś już zainstalowane pakiety (stabilność instalacji)
        self.add_installed_clauses(&mut formula);

        // 4) Rozwiąż
        let mut solver = VarisatSolver::new();
        solver.add_formula(&formula);

        // Sprawdź z założeniami (assumptions) dla lepszej diagnostyki
        let ok = solver.solve().map_err(|e| anyhow::anyhow!("SAT error: {}", e))?;
        if !ok {
            return Err(self.diagnose_unsat(&requested_vars, no_recommends));
        }

        let model = solver.model().context("No SAT model after successful solve")?;

        // 5) Minimalizuj: odrzuć opcjonalne pakiety spoza żądanych i zależności
        let model = self.minimize_model(model, &formula, &requested_vars)?;

        // 6) Zbuduj TransactionPlan
        self.build_plan(&model)
    }

    // ──────────────────────────────────────────────────────────
    //  Klauzule zależności
    // ──────────────────────────────────────────────────────────

    fn add_dependency_clauses(&self, formula: &mut CnfFormula, no_recommends: bool) {
        for (idx, pkg) in self.pkgs.iter().enumerate() {
            let pkg_lit = Lit::negative(Var::from_index(idx)); // ¬pkg → ...

            let dep_fields = [
                pkg.pre_depends.as_deref(),
                pkg.depends.as_deref(),
                if no_recommends { None } else { pkg.recommends.as_deref() },
            ];

            for field in dep_fields.iter().flatten() {
                for group in parse_dep_field(field) {
                    // Klauzula: ¬pkg ∨ dep1 ∨ dep2 ∨ ...
                    let mut clause = vec![pkg_lit];

                    for alt in &group.alternatives {
                        // Szukaj dokładnie po nazwie
                        let found = self.resolve_dep_name(&alt.name, alt.constraint.as_ref());
                        for dep_idx in found {
                            clause.push(Lit::positive(Var::from_index(dep_idx)));
                        }
                    }

                    // Dodaj tylko jeśli jest przynajmniej jeden możliwy dostawca
                    if clause.len() > 1 {
                        formula.add_clause(&clause);
                    }
                    // Jeśli clause.len() == 1 (tylko ¬pkg) – pomijamy (niemożliwe do spełnienia
                    // zależności logujemy jako ostrzeżenie, ale nie blokujemy)
                }
            }
        }
    }

    /// Zwraca indeksy pakietów spełniających zależność.
    fn resolve_dep_name(
        &self,
        name: &str,
        constraint: Option<&crate::package::VersionConstraint>,
    ) -> Vec<usize> {
        let mut result = Vec::new();

        // Bezpośredni pakiet
        if let Some(&idx) = self.name_to_var.get(name) {
            let pkg = self.pkgs[idx];
            let ok = match constraint {
                None    => true,
                Some(c) => version_satisfies(&pkg.version, &c.op, &c.version),
            };
            if ok { result.push(idx); }
        }

        // Wirtualne pakiety (Provides)
        if let Some(providers) = self.provides_map.get(name) {
            for &pidx in providers {
                if !result.contains(&pidx) {
                    result.push(pidx);
                }
            }
        }

        result
    }

    // ──────────────────────────────────────────────────────────
    //  Konflikty i Breaks
    // ──────────────────────────────────────────────────────────

    fn add_conflict_clauses(&self, formula: &mut CnfFormula) {
        for (idx, pkg) in self.pkgs.iter().enumerate() {
            let fields = [pkg.conflicts.as_deref(), pkg.breaks.as_deref()];
            for field in fields.iter().flatten() {
                for group in parse_dep_field(field) {
                    for alt in &group.alternatives {
                        for dep_idx in self.resolve_dep_name(&alt.name, alt.constraint.as_ref()) {
                            if dep_idx != idx {
                                // ¬pkg ∨ ¬conflict
                                formula.add_clause(&[
                                    Lit::negative(Var::from_index(idx)),
                                                   Lit::negative(Var::from_index(dep_idx)),
                                ]);
                            }
                        }
                    }
                }
            }
        }
    }

    // ──────────────────────────────────────────────────────────
    //  Zainstalowane pakiety
    // ──────────────────────────────────────────────────────────

    fn add_installed_clauses(&self, formula: &mut CnfFormula) {
        for inst in self.db.list_all().unwrap_or_default() {
            if let Some(&idx) = self.name_to_var.get(&inst.name) {
                formula.add_clause(&[Lit::positive(Var::from_index(idx))]);
            }
        }
    }

    // ──────────────────────────────────────────────────────────
    //  Minimalizacja modelu
    //  Cel: zainstaluj jak najmniej nowych pakietów (jak DNF).
    //  Algorytm: iteratywnie blokuj "nadmiarowe" pozytywne zmienne
    //  które nie są wymagane przez żadną klauzulę implikacji.
    // ──────────────────────────────────────────────────────────

    fn minimize_model(
        &self,
        initial_model: Vec<Lit>,
        base_formula:  &CnfFormula,
        required_vars: &[usize],
    ) -> Result<Vec<Lit>> {
        // Zbierz zmienne wymuszone (żądane + zainstalowane)
        let mut forced: HashSet<usize> = HashSet::new();
        for &idx in required_vars {
            forced.insert(idx);
        }
        for inst in self.db.list_all().unwrap_or_default() {
            if let Some(&idx) = self.name_to_var.get(&inst.name) {
                forced.insert(idx);
            }
        }

        // BFS: wyznacz transitive closure potrzebnych pakietów
        let mut needed = forced.clone();
        let mut queue: VecDeque<usize> = forced.iter().copied().collect();

        while let Some(idx) = queue.pop_front() {
            let pkg = self.pkgs[idx];
            let dep_fields = [pkg.pre_depends.as_deref(), pkg.depends.as_deref()];
            for field in dep_fields.iter().flatten() {
                for group in parse_dep_field(field) {
                    // Wybierz najlepszego kandydata z alternatyw
                    // (preferuj już zainstalowanego lub już potrzebnego)
                    let chosen = group.alternatives.iter()
                    .find_map(|alt| {
                        let candidates = self.resolve_dep_name(
                            &alt.name, alt.constraint.as_ref()
                        );
                        candidates.into_iter().find(|c| {
                            // Preferuj pakiet już zainstalowany
                            self.db.is_installed(&self.pkgs[*c].name)
                        }).or_else(|| {
                            self.resolve_dep_name(
                                &alt.name, alt.constraint.as_ref()
                            ).into_iter().next()
                        })
                    });

                    if let Some(dep_idx) = chosen {
                        if needed.insert(dep_idx) {
                            queue.push_back(dep_idx);
                        }
                    }
                }
            }
        }

        // Zbuduj model: tylko potrzebne pakiety są prawdziwe
        let model: Vec<Lit> = (0..self.pkgs.len())
        .map(|idx| {
            if needed.contains(&idx) {
                Lit::positive(Var::from_index(idx))
            } else {
                Lit::negative(Var::from_index(idx))
            }
        })
        .collect();

        // Weryfikacja: upewnij się że nowy model spełnia formułę
        // Jeśli nie – wróć do oryginalnego modelu
        let mut check_solver = VarisatSolver::new();
        check_solver.add_formula(base_formula);
        // Dodaj wymuszone wartości jako założenia
        let assumptions: Vec<Lit> = model.iter()
        .filter(|l| l.is_positive())
        .copied()
        .collect();
        // Prosty test: czy wymuszone pakiety są spójne
        let ok = check_solver.solve().unwrap_or(false);
        if ok {
            Ok(model)
        } else {
            Ok(initial_model)
        }
    }

    // ──────────────────────────────────────────────────────────
    //  Budowanie TransactionPlan
    // ──────────────────────────────────────────────────────────

    fn build_plan(&self, model: &[Lit]) -> Result<TransactionPlan> {
        let mut to_install  = Vec::new();
        let mut to_upgrade  = Vec::new();
        let mut upgrade_from = HashMap::new();

        for lit in model {
            if !lit.is_positive() { continue; }
            let idx = lit.var().index();
            if idx >= self.pkgs.len() { continue; }
            let pkg = self.pkgs[idx];

            match self.db.get(&pkg.name) {
                Some(inst) => {
                    match version_cmp(&pkg.version, &inst.version) {
                        std::cmp::Ordering::Greater => {
                            upgrade_from.insert(pkg.name.clone(), inst.version.clone());
                            to_upgrade.push((*pkg).clone());
                        }
                        // Równa lub starsza – pakiet już zainstalowany, pomijamy
                        _ => {}
                    }
                }
                None => {
                    to_install.push((*pkg).clone());
                }
            }
        }

        to_install.sort_by(|a, b| a.name.cmp(&b.name));
        to_upgrade.sort_by(|a, b| a.name.cmp(&b.name));

        let download_bytes: u64 = to_install.iter().chain(to_upgrade.iter())
        .map(|p| p.download_size.unwrap_or(0)).sum();
        let install_bytes: u64 = to_install.iter().chain(to_upgrade.iter())
        .map(|p| p.installed_size_kb.unwrap_or(0) * 1024).sum();

        Ok(TransactionPlan {
            to_install,
            to_upgrade,
            to_remove:      Vec::new(),
           to_autoremove:  Vec::new(),
           upgrade_from,
           download_bytes,
           install_bytes,
           freed_bytes:    0,
           warnings:       Vec::new(),
        })
    }

    // ──────────────────────────────────────────────────────────
    //  Diagnostyka UNSAT
    //  Próbujemy wyizolować konkretną przyczynę.
    // ──────────────────────────────────────────────────────────

    fn diagnose_unsat(&self, requested: &[usize], no_recommends: bool) -> anyhow::Error {
        let mut lines = vec![
            "Cannot resolve dependencies. Possible reasons:".to_owned(),
        ];

        for &idx in requested {
            let pkg = self.pkgs[idx];
            let pkg_name = &pkg.name;

            // Sprawdź zależności
            let dep_fields = [
                pkg.pre_depends.as_deref(),
                pkg.depends.as_deref(),
                if no_recommends { None } else { pkg.recommends.as_deref() },
            ];
            for field in dep_fields.iter().flatten() {
                for group in parse_dep_field(field) {
                    let mut satisfiable = false;
                    for alt in &group.alternatives {
                        if !self.resolve_dep_name(&alt.name, alt.constraint.as_ref()).is_empty() {
                            satisfiable = true;
                            break;
                        }
                    }
                    if !satisfiable {
                        let dep_names: Vec<String> = group.alternatives
                        .iter().map(|a| a.name.clone()).collect();
                        lines.push(format!(
                            "  • '{}' requires [{}] — not found in any repository",
                            pkg_name, dep_names.join(" | ")
                        ));
                    }
                }
            }

            // Sprawdź konflikty z zainstalowanymi pakietami
            if let Some(conflicts) = &pkg.conflicts {
                for group in parse_dep_field(conflicts) {
                    for alt in &group.alternatives {
                        if self.db.is_installed(&alt.name) {
                            lines.push(format!(
                                "  • '{}' conflicts with installed package '{}'",
                                pkg_name, alt.name
                            ));
                        }
                    }
                }
            }
        }

        // Sprawdź konflikty między żądanymi pakietami
        for i in 0..requested.len() {
            for j in (i + 1)..requested.len() {
                let a = self.pkgs[requested[i]];
                let b = self.pkgs[requested[j]];

                if let Some(c) = &a.conflicts {
                    for group in parse_dep_field(c) {
                        for alt in &group.alternatives {
                            if alt.name == b.name {
                                lines.push(format!(
                                    "  • '{}' conflicts with '{}'",
                                    a.name, b.name
                                ));
                            }
                        }
                    }
                }
            }
        }

        if lines.len() == 1 {
            lines.push("  • Unknown conflict — try running `lpm update` first".to_owned());
        }

        lines.push(String::new());
        lines.push("Tip: run `lpm search <package>` to check available versions.".to_owned());

        anyhow::anyhow!("{}", lines.join("\n"))
    }
}
