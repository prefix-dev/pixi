use rattler_conda_types::{
    BuildNumberSpec, NamedChannelOrUrl, ParseStrictness::Strict, StringMatcher, VersionSpec,
};
use rattler_digest::{Md5Hash, Sha256Hash};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use serde_with::serde_as;
use url::Url;

use crate::{DetailedSpec, GitReference, GitSpec, PathSpec, PixiSpec, UrlSpec};

#[serde_as]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpec {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub version: Option<VersionSpec>,

    /// The URL of the package
    pub url: Option<Url>,

    /// The git url of the package
    pub git: Option<Url>,

    /// The path to the package
    pub path: Option<String>,

    /// The git revision of the package
    pub branch: Option<String>,

    /// The git revision of the package
    pub rev: Option<String>,

    /// The git revision of the package
    pub tag: Option<String>,

    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub build: Option<StringMatcher>,

    /// The build number of the package
    pub build_number: Option<BuildNumberSpec>,

    /// Match the specific filename of the package
    pub file_name: Option<String>,

    /// The channel of the package
    pub channel: Option<NamedChannelOrUrl>,

    /// The subdir of the channel
    pub subdir: Option<String>,

    /// The md5 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
}

impl<'de> Deserialize<'de> for PixiSpec {
    fn deserialize<D>(deserializer: D) -> Result<PixiSpec, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .expecting(
                "a version string like \">=0.9.8\" or a detailed dependency like { version = \">=0.9.8\" }",
            )
            .string(|str| {
                VersionSpec::from_str(str, Strict)
                    .map_err(serde_untagged::de::Error::custom)
                    .map(PixiSpec::Version)
            })
            .map(|map| {
                let raw_spec: RawSpec = map.deserialize()?;

                if raw_spec.git.is_none()
                    && (raw_spec.branch.is_some()
                        || raw_spec.rev.is_some()
                        || raw_spec.tag.is_some())
                {
                    return Err(serde_untagged::de::Error::custom(
                        "`branch`, `rev`, and `tag` are only valid when `git` is specified",
                    ));
                }

                let is_git = raw_spec.git.is_some();
                let is_path = raw_spec.path.is_some();
                let is_url = raw_spec.url.is_some();


                let git_key = is_git.then_some("`git`");
                let path_key = is_path.then_some("`path`");
                let url_key = is_url.then_some("`url`");
                let non_detailed_keys = [git_key, path_key, url_key]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");

                if !non_detailed_keys.is_empty() && raw_spec.version.is_some() {
                    return Err(serde_untagged::de::Error::custom(
                        format!("`version` cannot be used with {non_detailed_keys}"),
                    ));
                }

                if !non_detailed_keys.is_empty() && raw_spec.build.is_some() {
                    return Err(serde_untagged::de::Error::custom(
                        format!("`build` cannot be used with {non_detailed_keys}"),
                    ));
                }

                if !non_detailed_keys.is_empty() && raw_spec.build_number.is_some() {
                    return Err(serde_untagged::de::Error::custom(
                        format!("`build` cannot be used with {non_detailed_keys}"),
                    ));
                }

                if !non_detailed_keys.is_empty() && raw_spec.file_name.is_some() {
                    return Err(serde_untagged::de::Error::custom(
                        format!("`build` cannot be used with {non_detailed_keys}"),
                    ));
                }

                if !non_detailed_keys.is_empty() && raw_spec.channel.is_some() {
                    return Err(serde_untagged::de::Error::custom(
                        format!("`build` cannot be used with {non_detailed_keys}"),
                    ));
                }

                if !non_detailed_keys.is_empty() && raw_spec.subdir.is_some() {
                    return Err(serde_untagged::de::Error::custom(
                        format!("`build` cannot be used with {non_detailed_keys}"),
                    ));
                }

                let non_url_keys = [git_key, path_key]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");
                if !non_url_keys.is_empty() && raw_spec.sha256.is_some() {
                    return Err(serde_untagged::de::Error::custom(
                        format!("`sha256` cannot be used with {non_url_keys}"),
                    ));
                }
                if !non_url_keys.is_empty() && raw_spec.md5.is_some() {
                    return Err(serde_untagged::de::Error::custom(
                        format!("`md5` cannot be used with {non_url_keys}"),
                    ));
                }

                let spec = match (raw_spec.url, raw_spec.path, raw_spec.git) {
                    (Some(url), None, None) => PixiSpec::Url(UrlSpec {
                        url,
                        md5: raw_spec.md5,
                        sha256: raw_spec.sha256,
                    }),
                    (None, Some(path), None) => PixiSpec::Path(PathSpec { path: path.into() }),
                    (None, None, Some(git)) => {
                        let rev = match (raw_spec.branch, raw_spec.rev, raw_spec.tag) {
                            (Some(branch), None, None) => Some(GitReference::Branch(branch)),
                            (None, Some(rev), None) => Some(GitReference::Rev(rev)),
                            (None, None, Some(tag)) => Some(GitReference::Tag(tag)),
                            (None, None, None) => None,
                            _ => {
                                return Err(serde_untagged::de::Error::custom(
                                    "only one of `branch`, `rev`, or `tag` can be specified",
                                ));
                            }
                        };
                        PixiSpec::Git(GitSpec { git, rev })
                    },
                    (None, None, None) => {
                        let is_detailed =
                            raw_spec.version.is_some() ||
                                raw_spec.build.is_some() ||
                                raw_spec.build_number.is_some() ||
                                raw_spec.file_name.is_some() ||
                                raw_spec.channel.is_some() ||
                                raw_spec.subdir.is_some() ||
                                raw_spec.md5.is_some() ||
                                raw_spec.sha256.is_some();
                        if !is_detailed {
                            return Err(serde_untagged::de::Error::custom(
                                "one of `version`, `build`, `build-number`, `file-name`, `channel`, `subdir`, `md5`, `sha256`, `git`, `url`, or `path` must be specified",
                            ))
                        }

                        PixiSpec::DetailedVersion(DetailedSpec {
                            version: raw_spec.version,
                            build: raw_spec.build,
                            build_number: raw_spec.build_number,
                            file_name: raw_spec.file_name,
                            channel: raw_spec.channel,
                            subdir: raw_spec.subdir,
                            md5: raw_spec.md5,
                            sha256: raw_spec.sha256,
                        })
                    }
                    (_, _, _) => {
                        return Err(serde_untagged::de::Error::custom(
                            "only one of `url`, `path`, or `git` can be specified",
                        ))
                    }
                };

                Ok(spec)
            })
            .deserialize(deserializer)
    }
}

impl<'de> Deserialize<'de> for PathSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            path: String,
        }

        Raw::deserialize(deserializer).map(|raw| PathSpec {
            path: raw.path.into(),
        })
    }
}

impl Serialize for PathSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Raw {
            path: String,
        }

        Raw {
            path: self.path.to_string(),
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
mod test {
    use serde::Serialize;
    use serde_json::{json, Value};

    use super::*;

    #[test]
    fn test_round_trip() {
        let examples = [
            json! { "1.2.3" },
            json!({ "version": "1.2.3" }),
            json!({ "ver": "1.2.3" }),
            json! { "*" },
            json!({ "path": "foobar" }),
            json!({ "path": "foobar", "version": "1.2.3" }),
            json!({ "subdir": "linux-64" }),
            json!({ "channel": "conda-forge", "subdir": "linux-64" }),
            json!({ "channel": "conda-forge", "subdir": "linux-64" }),
            json!({ "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "version": "//" }),
            json!({ "path": "foobar", "version": "//" }),
            json!({ "path": "foobar", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "path": "foobar", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "version": "1.2.3", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda" }),
            json!({ "url": "https://conda.anaconda.org/conda-forge/linux-64/21cmfast-3.3.1-py38h0db86a8_1.conda", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "branch": "main", "tag": "v1" }),
            json!({ "git": "https://github.com/conda-forge/21cmfast-feedstock", "sha256": "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3" }),
        ];

        #[derive(Serialize)]
        struct Snapshot {
            input: Value,
            result: Value,
        }

        let mut snapshot = Vec::new();
        for input in examples {
            let spec: Result<PixiSpec, _> = serde_json::from_value(input.clone());
            let result = match spec {
                Ok(spec) => {
                    let spec = PixiSpec::from(spec);
                    serde_json::to_value(&spec).unwrap()
                }
                Err(e) => {
                    json!({
                        "error": format!("ERROR: {e}")
                    })
                }
            };

            snapshot.push(Snapshot { input, result });
        }

        insta::assert_yaml_snapshot!(snapshot);
    }
}
