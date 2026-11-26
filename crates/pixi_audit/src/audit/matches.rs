use std::collections::{BTreeSet, HashMap};

use crate::report::Match;

/// Coalesces documents by id or alias within one queried package.
/// An alias may connect two groups only after both have been seen.
pub(super) struct MatchGroups {
    groups: Vec<Match>,
    by_id: HashMap<String, usize>,
}

impl MatchGroups {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            groups: Vec::with_capacity(capacity),
            by_id: HashMap::new(),
        }
    }

    pub(super) fn push(&mut self, vuln_match: Match) {
        let matches: BTreeSet<usize> = vuln_match
            .ids()
            .filter_map(|id| self.by_id.get(&id.to_ascii_lowercase()).copied())
            .collect();
        let index = if let Some(&index) = matches.first() {
            self.groups[index].merge_from(vuln_match);
            // Removing from the end leaves the surviving group at `index`.
            for other in matches.into_iter().skip(1).rev() {
                let previous = self.groups.swap_remove(other);
                if other < self.groups.len() {
                    for id in self.groups[other].ids() {
                        self.by_id.insert(id.to_ascii_lowercase(), other);
                    }
                }
                self.groups[index].merge_from(previous);
            }
            index
        } else {
            let index = self.groups.len();
            self.groups.push(vuln_match);
            index
        };
        for id in self.groups[index].ids() {
            self.by_id.insert(id.to_ascii_lowercase(), index);
        }
    }

    pub(super) fn into_matches(self) -> impl Iterator<Item = Match> {
        self.groups.into_iter()
    }
}

impl Match {
    fn ids(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.id.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    /// Merge another vulnerability match for the same package before ignores and threat data are applied.
    fn merge_from(&mut self, other: Match) {
        self.aliases.push(other.id);
        self.aliases.extend(other.aliases);
        self.aliases.retain(|id| !id.eq_ignore_ascii_case(&self.id));
        self.aliases.sort_unstable();
        self.aliases.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        self.severity = self.severity.max(other.severity);
        self.cvss_base_score = match (self.cvss_base_score, other.cvss_base_score) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        for fixed in other.fixed_versions {
            if !self.fixed_versions.contains(&fixed) {
                self.fixed_versions.push(fixed);
            }
        }
        self.advisories.extend(other.advisories);
        for fixed in other.other_ecosystem_fixes {
            if !self.other_ecosystem_fixes.contains(&fixed) {
                self.other_ecosystem_fixes.push(fixed);
            }
        }
        for release in other.upstream_releases {
            if !self.upstream_releases.contains(&release) {
                self.upstream_releases.push(release);
            }
        }
        self.match_confidence = self.match_confidence.min(other.match_confidence);
        if self.matched_identity != other.matched_identity {
            self.matched_identity = None;
        }
        self.summary = self.summary.take().or(other.summary);
        self.url = self.url.take().or(other.url);
    }
}
