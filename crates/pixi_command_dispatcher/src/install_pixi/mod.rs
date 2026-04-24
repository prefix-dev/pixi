mod ext;
pub(crate) mod reporter;

pub use ext::InstallPixiEnvironmentExt;

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsStr,
    path::PathBuf,
    sync::Arc,
};

use miette::Diagnostic;

use pixi_record::{UnresolvedPixiRecord, VariantValue};
use pixi_spec::ResolvedExcludeNewer;
use rattler::install::{
    InstallationResultRecord, InstallerError, Transaction,
    link_script::{LinkScriptError, PrePostLinkResult},
};
use rattler_conda_types::{ChannelUrl, PackageName, PrefixRecord, RepoDataRecord, prefix::Prefix};
use thiserror::Error;

use crate::{BuildEnvironment, SourceBuildError};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallPixiEnvironmentSpec {
    pub name: String,

    /// Records to install; partial source records are built from source.
    #[serde(skip)]
    pub records: Vec<UnresolvedPixiRecord>,

    /// Packages neither removed when missing from `records` nor updated
    /// when already installed.
    pub ignore_packages: Option<HashSet<PackageName>>,

    #[serde(skip)]
    pub prefix: Prefix,

    #[serde(skip)]
    pub installed: Option<Vec<PrefixRecord>>,

    pub build_environment: BuildEnvironment,

    #[serde(skip_serializing_if = "HashSet::is_empty")]
    pub force_reinstall: HashSet<rattler_conda_types::PackageName>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<ResolvedExcludeNewer>,

    pub channels: Vec<ChannelUrl>,

    pub variant_configuration: Option<BTreeMap<String, Vec<VariantValue>>>,

    pub variant_files: Option<Vec<PathBuf>>,
}

pub struct InstallPixiEnvironmentResult {
    pub transaction: Transaction<InstallationResultRecord, RepoDataRecord>,

    /// `None` when link scripts were disabled.
    pub pre_link_script_result: Option<PrePostLinkResult>,

    /// `None` when link scripts were disabled.
    pub post_link_script_result: Option<Result<PrePostLinkResult, LinkScriptError>>,

    /// Built repodata records for source records present in the input.
    pub resolved_source_records: HashMap<PackageName, Arc<RepoDataRecord>>,
}

impl InstallPixiEnvironmentSpec {
    pub fn new(
        records: impl IntoIterator<Item = impl Into<UnresolvedPixiRecord>>,
        prefix: Prefix,
    ) -> Self {
        let records = records.into_iter().map(Into::into).collect();
        InstallPixiEnvironmentSpec {
            name: prefix
                .file_name()
                .map(OsStr::to_string_lossy)
                .map(Cow::into_owned)
                .unwrap_or_default(),
            records,
            prefix,
            installed: None,
            ignore_packages: None,
            build_environment: BuildEnvironment::default(),
            force_reinstall: HashSet::new(),
            exclude_newer: None,
            channels: Vec::new(),
            variant_configuration: None,
            variant_files: None,
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum InstallPixiEnvironmentError {
    #[error("failed to collect prefix records from '{}'", .0.path().display())]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    ReadInstalledPackages(Prefix, #[source] std::io::Error),

    #[error(transparent)]
    Installer(InstallerError),

    #[error("failed to build '{}' from '{}'",
        .0.as_source(),
        .1)]
    BuildUnresolvedSourceError(
        PackageName,
        Box<pixi_record::PinnedSourceSpec>,
        #[diagnostic_source]
        #[source]
        SourceBuildError,
    ),

    #[error("failed to clear source-build cache for '{}'", .0.as_source())]
    ClearSourceBuildCache(PackageName, #[source] std::io::Error),

    #[error(
        "failed to convert install transaction to prefix records from '{}'",
        .0.path().display()
    )]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    ConvertTransactionToPrefixRecord(Prefix, #[source] std::io::Error),
}
