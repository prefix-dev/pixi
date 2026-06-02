//! Compute-engine-driven helper for walking the input globs a backend
//! reported.  Both the flat `input_globs: Vec<String>` and the
//! structured `input_glob_sets: Option<Vec<InputGlobSet>>` are consumed:
//! the flat form is folded into a synthetic [`InputGlobSet`] (no
//! markers, default hidden handling, caller's root) and walked
//! alongside any structured groups via [`InputGlobSetWalkKey`], so two
//! consumers that arrive at the same `(absolute_root, patterns,
//! markers, exclude_hidden)` tuple share a single walk for the engine's
//! lifetime.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use pixi_build_types::InputGlobSet;
use pixi_compute_engine::ComputeCtx;
use pixi_glob::GlobSetError;

use crate::keys::InputGlobSetWalkKey;

/// Walk every group implied by the input declaration, deduping the
/// resulting paths.  See module docs for the consumption rules.
pub async fn collect_input_files_via_engine(
    ctx: &mut ComputeCtx,
    input_globs: &[String],
    input_glob_sets: Option<&[InputGlobSet]>,
    caller_root: &Path,
) -> Result<Vec<PathBuf>, Arc<GlobSetError>> {
    let mut all: Vec<PathBuf> = Vec::new();

    if !input_globs.is_empty() {
        let flat_as_group = InputGlobSet {
            patterns: input_globs.to_vec(),
            markers: Vec::new(),
            exclude_hidden: true,
            root: None,
        };
        let key = InputGlobSetWalkKey::from_group(&flat_as_group, caller_root);
        let paths = ctx.compute(&key).await?;
        all.extend(paths.iter().cloned());
    }

    if let Some(groups) = input_glob_sets {
        for group in groups {
            let key = InputGlobSetWalkKey::from_group(group, caller_root);
            let paths = ctx.compute(&key).await?;
            all.extend(paths.iter().cloned());
        }
    }

    // Dedupe; a backend transitioning to `input_glob_sets` emits both
    // forms for back-compat and we'd otherwise count overlapping matches
    // twice.
    all.sort();
    all.dedup();
    Ok(all)
}
