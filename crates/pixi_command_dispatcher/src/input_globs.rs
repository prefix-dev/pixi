//! Compute-engine-driven helper for walking the input glob groups a
//! backend reported.  Each [`InputGlobSet`] is walked via
//! [`InputGlobSetWalkKey`], so two consumers that arrive at the same
//! `(absolute_root, patterns, markers, exclude_hidden)` tuple share a
//! single walk for the engine's lifetime.
//!
//! Backends report inputs as a flat `input_globs: Vec<String>` plus an
//! optional structured `input_glob_sets`. [`fold_input_globs`] normalizes
//! the two into a single `Vec<InputGlobSet>` (the flat list becomes one
//! marker-free, hidden-excluding group) so the rest of the pipeline only
//! ever deals with groups.

use std::sync::Arc;

use pixi_build_types::InputGlobSet;
use pixi_compute_engine::ComputeCtx;
use pixi_glob::GlobSetError;
use pixi_path::{AbsPath, AbsPathBuf};

use crate::keys::InputGlobSetWalkKey;

/// Normalize a backend's `(input_globs, input_glob_sets)` pair into a
/// single list of groups. The flat globs (if any) are folded into one
/// group with default config (no markers, hidden excluded, caller's root).
pub fn fold_input_globs(
    input_globs: Vec<String>,
    input_glob_sets: Option<Vec<InputGlobSet>>,
) -> Vec<InputGlobSet> {
    let mut groups = input_glob_sets.unwrap_or_default();
    if !input_globs.is_empty() {
        groups.push(InputGlobSet {
            patterns: input_globs,
            markers: Vec::new(),
            exclude_hidden: true,
            root: None,
        });
    }
    groups
}

/// Walk every group from the absolute `caller_root`, deduping the resulting
/// (absolute) paths. Groups are independent walks and run concurrently.
/// Overlapping matches across groups (e.g. a flat group folded alongside a
/// structured one) are collapsed.
pub async fn collect_input_files(
    ctx: &mut ComputeCtx,
    groups: &[InputGlobSet],
    caller_root: &AbsPath,
) -> Result<Vec<AbsPathBuf>, Arc<GlobSetError>> {
    let caller_root = caller_root.to_path_buf();
    let per_group = ctx
        .try_compute_join(
            groups.to_vec(),
            async move |sub_ctx: &mut ComputeCtx,
                        group: InputGlobSet|
                        -> Result<Vec<AbsPathBuf>, Arc<GlobSetError>> {
                let key = InputGlobSetWalkKey::from_group(&group, caller_root.as_std_path());
                let paths = sub_ctx.compute(&key).await?;
                Ok(paths
                    .iter()
                    .map(|path| {
                        // The walk root is absolute, so every match is too.
                        AbsPathBuf::new(path.clone())
                            .expect("glob walk of an absolute root yields absolute paths")
                    })
                    .collect())
            },
        )
        .await?;

    let mut all: Vec<AbsPathBuf> = per_group.into_iter().flatten().collect();
    all.sort();
    all.dedup();
    Ok(all)
}

#[cfg(test)]
mod tests {
    use pixi_compute_engine::ComputeEngine;
    use pixi_path::AbsPath;
    use tempfile::TempDir;

    use super::*;

    fn group(patterns: &[&str], exclude_hidden: bool) -> InputGlobSet {
        InputGlobSet {
            patterns: patterns.iter().map(|p| p.to_string()).collect(),
            markers: Vec::new(),
            exclude_hidden,
            root: None,
        }
    }

    /// Drive `collect_input_files` (needs a `ComputeCtx`) through a throwaway
    /// engine and return the matched paths.
    async fn collect(groups: &[InputGlobSet], root: &AbsPath) -> Vec<AbsPathBuf> {
        ComputeEngine::new()
            .with_ctx(async |ctx| collect_input_files(ctx, groups, root).await)
            .await
            .expect("no cycle")
            .expect("walk succeeds")
    }

    #[test]
    fn fold_normalizes_flat_and_structured_into_groups() {
        // Empty in, empty out.
        assert!(fold_input_globs(Vec::new(), None).is_empty());

        // Flat globs become one default group (no markers, hidden excluded,
        // caller's root).
        let folded = fold_input_globs(vec!["a".into(), "b".into()], None);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].patterns, vec!["a".to_string(), "b".to_string()]);
        assert!(folded[0].markers.is_empty());
        assert!(folded[0].exclude_hidden);
        assert!(folded[0].root.is_none());

        // Structured groups pass through unchanged.
        let structured = vec![group(&["x"], false)];
        assert_eq!(
            fold_input_globs(Vec::new(), Some(structured.clone())),
            structured
        );

        // Both: structured first, the folded flat group appended.
        let folded = fold_input_globs(vec!["flat".into()], Some(vec![group(&["x"], true)]));
        assert_eq!(folded.len(), 2);
        assert_eq!(folded[0].patterns, vec!["x".to_string()]);
        assert_eq!(folded[1].patterns, vec!["flat".to_string()]);
    }

    #[tokio::test]
    async fn collect_returns_absolute_matches() {
        let tmp = TempDir::new().unwrap();
        fs_err::write(tmp.path().join("a.txt"), b"x").unwrap();
        let root = AbsPath::new(tmp.path()).unwrap();

        let files = collect(&[group(&["**"], true)], root).await;
        assert!(files.iter().all(|p| p.as_std_path().is_absolute()));
        assert!(files.iter().any(|p| p.as_std_path().ends_with("a.txt")));
    }

    #[tokio::test]
    async fn collect_honors_structured_group_config() {
        // Proves the per-group config (here `exclude_hidden`) is threaded
        // through to the walk rather than ignored.
        let tmp = TempDir::new().unwrap();
        fs_err::write(tmp.path().join(".hidden"), b"x").unwrap();
        let root = AbsPath::new(tmp.path()).unwrap();

        let excluded = collect(&[group(&["**"], true)], root).await;
        assert!(
            !excluded
                .iter()
                .any(|p| p.as_std_path().ends_with(".hidden"))
        );

        let included = collect(&[group(&["**"], false)], root).await;
        assert!(
            included
                .iter()
                .any(|p| p.as_std_path().ends_with(".hidden"))
        );
    }

    #[tokio::test]
    async fn collect_unions_and_dedups_multiple_groups() {
        let tmp = TempDir::new().unwrap();
        fs_err::write(tmp.path().join("a.txt"), b"x").unwrap();
        fs_err::write(tmp.path().join("b.log"), b"x").unwrap();
        let root = AbsPath::new(tmp.path()).unwrap();

        // Two overlapping groups; `a.txt` is matched by both and must appear once.
        let files = collect(&[group(&["**/*.txt"], true), group(&["**"], true)], root).await;
        assert_eq!(
            files
                .iter()
                .filter(|p| p.as_std_path().ends_with("a.txt"))
                .count(),
            1,
            "overlapping matches across groups must be deduped",
        );
        assert!(files.iter().any(|p| p.as_std_path().ends_with("b.log")));
    }
}
