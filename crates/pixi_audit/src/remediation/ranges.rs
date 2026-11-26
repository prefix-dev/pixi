//! Sets of versions as sorted, disjoint intervals, for any totally ordered
//! version type.

use std::{cmp::Ordering, ops::Bound};

/// The versions between two bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Interval<V> {
    pub lo: Bound<V>,
    pub hi: Bound<V>,
}

impl<V: Ord> Interval<V> {
    fn is_empty(&self) -> bool {
        match (&self.lo, &self.hi) {
            (Bound::Unbounded, _) | (_, Bound::Unbounded) => false,
            (Bound::Included(lo), Bound::Included(hi)) => lo > hi,
            (
                Bound::Included(lo) | Bound::Excluded(lo),
                Bound::Included(hi) | Bound::Excluded(hi),
            ) => lo >= hi,
        }
    }

    #[cfg(test)]
    fn contains(&self, version: &V) -> bool {
        let above = match &self.lo {
            Bound::Unbounded => true,
            Bound::Included(lo) => version >= lo,
            Bound::Excluded(lo) => version > lo,
        };
        let below = match &self.hi {
            Bound::Unbounded => true,
            Bound::Included(hi) => version <= hi,
            Bound::Excluded(hi) => version < hi,
        };
        above && below
    }
}

/// Orders bounds as the start of an interval.
pub(crate) fn cmp_lo<V: Ord>(a: &Bound<V>, b: &Bound<V>) -> Ordering {
    match (a, b) {
        (Bound::Unbounded, Bound::Unbounded) => Ordering::Equal,
        (Bound::Unbounded, _) => Ordering::Less,
        (_, Bound::Unbounded) => Ordering::Greater,
        (Bound::Included(a), Bound::Included(b)) | (Bound::Excluded(a), Bound::Excluded(b)) => {
            a.cmp(b)
        }
        (Bound::Included(a), Bound::Excluded(b)) => a.cmp(b).then(Ordering::Less),
        (Bound::Excluded(a), Bound::Included(b)) => a.cmp(b).then(Ordering::Greater),
    }
}

fn cmp_hi<V: Ord>(a: &Bound<V>, b: &Bound<V>) -> Ordering {
    match (a, b) {
        (Bound::Unbounded, Bound::Unbounded) => Ordering::Equal,
        (Bound::Unbounded, _) => Ordering::Greater,
        (_, Bound::Unbounded) => Ordering::Less,
        (Bound::Included(a), Bound::Included(b)) | (Bound::Excluded(a), Bound::Excluded(b)) => {
            a.cmp(b)
        }
        (Bound::Included(a), Bound::Excluded(b)) => a.cmp(b).then(Ordering::Greater),
        (Bound::Excluded(a), Bound::Included(b)) => a.cmp(b).then(Ordering::Less),
    }
}

/// Whether an interval ending at `hi` and a later-starting one starting at
/// `lo` leave no version between them.
fn joins<V: Ord>(hi: &Bound<V>, lo: &Bound<V>) -> bool {
    match (hi, lo) {
        (Bound::Unbounded, _) | (_, Bound::Unbounded) => true,
        (Bound::Excluded(hi), Bound::Excluded(lo)) => lo < hi,
        (Bound::Included(hi) | Bound::Excluded(hi), Bound::Included(lo) | Bound::Excluded(lo)) => {
            lo <= hi
        }
    }
}

/// The bound on the other side of the same boundary: the versions below an
/// interval starting at `[v` end at `v)`.
fn flip<V: Clone>(bound: &Bound<V>) -> Option<Bound<V>> {
    match bound {
        Bound::Unbounded => None,
        Bound::Included(version) => Some(Bound::Excluded(version.clone())),
        Bound::Excluded(version) => Some(Bound::Included(version.clone())),
    }
}

/// A set of versions as sorted, disjoint intervals with gaps between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntervalSet<V>(Vec<Interval<V>>);

impl<V: Ord + Clone> IntervalSet<V> {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn full() -> Self {
        Self(vec![Interval {
            lo: Bound::Unbounded,
            hi: Bound::Unbounded,
        }])
    }

    pub fn point(version: V) -> Self {
        Self(vec![Interval {
            lo: Bound::Included(version.clone()),
            hi: Bound::Included(version),
        }])
    }

    pub fn from_intervals(intervals: impl IntoIterator<Item = Interval<V>>) -> Self {
        let mut intervals: Vec<_> = intervals
            .into_iter()
            .filter(|interval| !interval.is_empty())
            .collect();
        intervals.sort_by(|a, b| cmp_lo(&a.lo, &b.lo));
        let mut merged: Vec<Interval<V>> = Vec::with_capacity(intervals.len());
        for interval in intervals {
            match merged.last_mut() {
                Some(last) if joins(&last.hi, &interval.lo) => {
                    if cmp_hi(&interval.hi, &last.hi) == Ordering::Greater {
                        last.hi = interval.hi;
                    }
                }
                _ => merged.push(interval),
            }
        }
        Self(merged)
    }

    #[cfg(test)]
    pub fn intervals(&self) -> &[Interval<V>] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    pub fn contains(&self, version: &V) -> bool {
        self.0.iter().any(|interval| interval.contains(version))
    }

    pub fn union(&self, other: &Self) -> Self {
        Self::from_intervals(self.0.iter().chain(&other.0).cloned())
    }

    pub fn complement(&self) -> Self {
        let mut gaps = Vec::with_capacity(self.0.len() + 1);
        let mut start = Some(Bound::Unbounded);
        for interval in &self.0 {
            if let (Some(lo), Some(hi)) = (start.take(), flip(&interval.lo)) {
                gaps.push(Interval { lo, hi });
            }
            start = flip(&interval.hi);
        }
        if let Some(lo) = start {
            gaps.push(Interval {
                lo,
                hi: Bound::Unbounded,
            });
        }
        Self::from_intervals(gaps)
    }

    pub fn intersect(&self, other: &Self) -> Self {
        self.complement().union(&other.complement()).complement()
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        !self.intersect(other).is_empty()
    }

    /// The set as runs of intervals separated only by single excluded
    /// versions, which constraint syntaxes can express with `!=`.
    pub fn components(&self) -> Vec<Component<V>> {
        let mut components: Vec<Component<V>> = Vec::new();
        for interval in &self.0 {
            if let Some(last) = components.last_mut()
                && let (Bound::Excluded(end), Bound::Excluded(start)) = (&last.hi, &interval.lo)
                && end == start
            {
                last.excluded.push(start.clone());
                last.hi = interval.hi.clone();
                continue;
            }
            components.push(Component {
                lo: interval.lo.clone(),
                hi: interval.hi.clone(),
                excluded: Vec::new(),
            });
        }
        components
    }
}

/// A range of versions without the listed single versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Component<V> {
    pub lo: Bound<V>,
    pub hi: Bound<V>,
    pub excluded: Vec<V>,
}

impl<V: Ord + Clone> Component<V> {
    pub fn as_set(&self) -> IntervalSet<V> {
        let range = IntervalSet::from_intervals([Interval {
            lo: self.lo.clone(),
            hi: self.hi.clone(),
        }]);
        let excluded =
            IntervalSet::from_intervals(self.excluded.iter().cloned().map(|version| Interval {
                lo: Bound::Included(version.clone()),
                hi: Bound::Included(version),
            }));
        range.intersect(&excluded.complement())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed_open(lo: u32, hi: u32) -> Interval<u32> {
        Interval {
            lo: Bound::Included(lo),
            hi: Bound::Excluded(hi),
        }
    }

    #[test]
    fn contiguous_intervals_merge_but_a_missing_point_does_not() {
        let set = IntervalSet::from_intervals([closed_open(1, 3), closed_open(3, 5)]);
        assert_eq!(set.intervals(), &[closed_open(1, 5)]);

        let gap = IntervalSet::from_intervals([
            Interval {
                lo: Bound::Included(1),
                hi: Bound::Excluded(3),
            },
            Interval {
                lo: Bound::Excluded(3),
                hi: Bound::Excluded(5),
            },
        ]);
        assert_eq!(gap.intervals().len(), 2);
        assert!(!gap.contains(&3));
        assert_eq!(
            gap.components(),
            vec![Component {
                lo: Bound::Included(1),
                hi: Bound::Excluded(5),
                excluded: vec![3],
            }]
        );
    }

    #[test]
    fn complement_flips_every_boundary() {
        let affected = IntervalSet::from_intervals([
            Interval {
                lo: Bound::Unbounded,
                hi: Bound::Excluded(2),
            },
            Interval {
                lo: Bound::Included(5),
                hi: Bound::Included(7),
            },
        ]);
        let safe = affected.complement();
        assert_eq!(
            safe.intervals(),
            &[
                closed_open(2, 5),
                Interval {
                    lo: Bound::Excluded(7),
                    hi: Bound::Unbounded,
                },
            ]
        );
        assert_eq!(safe.complement(), affected);
        assert!(IntervalSet::<u32>::full().complement().is_empty());
        assert_eq!(
            IntervalSet::<u32>::empty().complement(),
            IntervalSet::full()
        );
    }

    #[test]
    fn intersection_keeps_only_shared_versions() {
        let a = IntervalSet::from_intervals([closed_open(1, 5)]);
        let b = IntervalSet::from_intervals([closed_open(4, 9)]);
        assert_eq!(a.intersect(&b).intervals(), &[closed_open(4, 5)]);
        assert!(!a.overlaps(&IntervalSet::from_intervals([closed_open(5, 6)])));
        assert!(a.overlaps(&IntervalSet::point(1)));
    }
}
