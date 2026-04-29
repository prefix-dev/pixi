use std::{collections::BTreeMap, sync::Arc};

use pixi_command_dispatcher::{
    CommandDispatcher, CommandDispatcherError, ComputeResultExt, EnvironmentRef,
    InstalledSourceHints, PtrArc, WorkspaceEnvRef,
};
use pixi_record::{PinnedSourceSpec, UnresolvedPixiRecord, UnresolvedSourceRecord};
use rattler_conda_types::PackageName;
use rattler_lock::FileFormatVersion;

use super::key::{LegacySourceEnv, LegacySourceEnvError, LegacySourceEnvKey};

/// Populate `build_packages` and `host_packages` on every source
/// record in `records` for lock files predating v7. v7+ lock files
/// already round-trip those envs through
/// [`LockFileResolver`](pixi_record::LockFileResolver), so this is a
/// no-op then.
///
/// Each source record is reified by dispatching
/// [`LegacySourceEnvKey`], which itself drives
/// [`ResolveSourcePackageKey`](pixi_command_dispatcher::keys::ResolveSourcePackageKey)
/// to recompute the envs from the build backend. Dispatches happen in
/// parallel; the compute engine dedups across records that share a
/// source location.
pub async fn reify_legacy_source_envs(
    command_dispatcher: &CommandDispatcher,
    records: &mut [UnresolvedPixiRecord],
    lock_file_version: FileFormatVersion,
    workspace_env_ref: &WorkspaceEnvRef,
) -> Result<(), CommandDispatcherError<LegacySourceEnvError>> {
    if lock_file_version >= FileFormatVersion::V7 {
        return Ok(());
    }

    // Indices of source records in `records`. We use parallel index
    // ordering so the `try_compute_join` results map back 1:1 to the
    // source record positions for write-back.
    let source_indices: Vec<usize> = records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| matches!(r, UnresolvedPixiRecord::Source(_)).then_some(i))
        .collect();
    if source_indices.is_empty() {
        return Ok(());
    }

    // Default `preferred_build_source` to every locked source record's
    // `build_source` pin so nested resolutions stay stable across
    // re-locks. Same default the v7 update path uses (see
    // `lock_file::update::pin_overrides`).
    let pin_overrides: BTreeMap<PackageName, PinnedSourceSpec> = records
        .iter()
        .filter_map(|r| match r {
            UnresolvedPixiRecord::Source(src) => src
                .build_source
                .clone()
                .map(|spec| (src.name().clone(), spec.into_pinned())),
            _ => None,
        })
        .collect();
    let pin_overrides = Arc::new(pin_overrides);

    // No prior resolution exists for a v6 lock file's build/host envs,
    // so we hand the recursion an empty hint set. `PtrArc::default()`
    // shares a process-wide singleton so the in-memory dedup at the
    // Key boundary still hits across invocations.
    let installed_source_hints = PtrArc::<InstalledSourceHints>::default();

    let env_ref = EnvironmentRef::Workspace(workspace_env_ref.clone());

    // Build one Key per source record, then dispatch all of them in
    // parallel inside a single `with_ctx` scope. Sharing a scope keeps
    // dedup live across records that target the same source location
    // (different variants of the same package).
    let keys: Vec<LegacySourceEnvKey> = source_indices
        .iter()
        .map(|&i| {
            let src = match &records[i] {
                UnresolvedPixiRecord::Source(s) => s,
                _ => unreachable!("source_indices was filtered to Source variants"),
            };
            LegacySourceEnvKey {
                package: src.name().clone(),
                manifest_source: src.manifest_source.clone(),
                build_source: src.build_source.clone(),
                variants: src.variants.clone(),
                env_ref: env_ref.clone(),
                preferred_build_source: pin_overrides.clone(),
                installed_source_hints: installed_source_hints.clone(),
            }
        })
        .collect();

    let envs: Vec<LegacySourceEnv> = command_dispatcher
        .engine()
        .with_ctx(async |ctx| {
            ctx.try_compute_join(keys, async |ctx, key| ctx.compute(&key).await)
                .await
        })
        .await
        .map_err_into_dispatcher(std::convert::identity)?;

    // Write the populated envs back into the source records. Each
    // source record is owned via Arc, so we clone-and-rewrap rather
    // than mutate through the Arc.
    for (idx, env) in source_indices.into_iter().zip(envs) {
        let UnresolvedPixiRecord::Source(arc) = &records[idx] else {
            unreachable!("source_indices was filtered to Source variants")
        };
        let updated = with_envs(arc, env);
        records[idx] = UnresolvedPixiRecord::Source(Arc::new(updated));
    }

    Ok(())
}

/// Clone an [`UnresolvedSourceRecord`] and replace its
/// `build_packages` / `host_packages` with the freshly-resolved envs.
fn with_envs(record: &Arc<UnresolvedSourceRecord>, env: LegacySourceEnv) -> UnresolvedSourceRecord {
    UnresolvedSourceRecord {
        data: record.data.clone(),
        manifest_source: record.manifest_source.clone(),
        build_source: record.build_source.clone(),
        variants: record.variants.clone(),
        identifier_hash: record.identifier_hash.clone(),
        build_packages: env.build_packages,
        host_packages: env.host_packages,
    }
}
