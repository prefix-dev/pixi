use std::{collections::HashMap, str::FromStr};

use pixi_build_types::{GitReference, GitSpec, SourcePackageSpec, UrlSpec};
use url::Url;

/// An internal type that supports converting a source dependency into a valid
/// URL and back.
///
/// This type is only used internally, it is not serialized. Therefore,
/// stability of how the URL is encoded is not important.
pub(crate) struct EncodedSourceSpecUrl(Url);

impl From<EncodedSourceSpecUrl> for Url {
    fn from(value: EncodedSourceSpecUrl) -> Self {
        value.0
    }
}

impl From<Url> for EncodedSourceSpecUrl {
    fn from(url: Url) -> Self {
        // Ensure the URL is a file URL
        assert_eq!(url.scheme(), "source", "URL must be a file URL");
        Self(url)
    }
}

impl From<EncodedSourceSpecUrl> for SourcePackageSpec {
    fn from(value: EncodedSourceSpecUrl) -> Self {
        let url = value.0;
        assert_eq!(url.scheme(), "source", "URL must be a file URL");
        let mut pairs: HashMap<_, _> = url.query_pairs().collect();
        if let Some(path) = pairs.remove("path") {
            pixi_build_types::PathSpec {
                path: path.into_owned(),
            }
            .into()
        } else if let Some(url) = pairs.remove("url") {
            let url = Url::from_str(&url).expect("must be a valid URL");
            let md5 = pairs
                .remove("md5")
                .and_then(|s| rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>(&s));
            let sha256 = pairs
                .remove("sha256")
                .and_then(|s| rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(&s));
            let subdirectory = pairs.remove("subdirectory").map(|s| s.into_owned());
            UrlSpec {
                url,
                md5,
                sha256,
                subdirectory,
            }
            .into()
        } else if let Some(git) = pairs.remove("git") {
            let git_url = Url::from_str(&git).expect("must be a valid URL");
            let rev = if let Some(rev) = pairs.remove("rev") {
                Some(GitReference::Rev(rev.into_owned()))
            } else if let Some(branch) = pairs.remove("branch") {
                Some(GitReference::Branch(branch.into_owned()))
            } else {
                pairs
                    .remove("tag")
                    .map(|tag| GitReference::Tag(tag.into_owned()))
            };

            let subdirectory = pairs.remove("subdirectory").map(|s| s.into_owned());
            GitSpec {
                git: git_url,
                rev,
                subdirectory,
            }
            .into()
        } else {
            panic!("URL must contain either 'path', 'url', or 'git' query parameters");
        }
    }
}

impl From<SourcePackageSpec> for EncodedSourceSpecUrl {
    fn from(value: SourcePackageSpec) -> Self {
        let mut url = Url::from_str("source://").expect("must be a valid URL");
        let mut query_pairs = url.query_pairs_mut();
        match value.location {
            pixi_build_types::SourcePackageLocationSpec::Url(url_spec) => {
                query_pairs.append_pair("url", url_spec.url.as_str());
                if let Some(md5) = &url_spec.md5 {
                    query_pairs.append_pair("md5", &format!("{md5:x}"));
                }
                if let Some(sha256) = &url_spec.sha256 {
                    query_pairs.append_pair("sha256", &format!("{sha256:x}"));
                }
                if let Some(subdirectory) = &url_spec.subdirectory {
                    query_pairs.append_pair("subdirectory", subdirectory);
                }
            }
            pixi_build_types::SourcePackageLocationSpec::Git(git) => {
                query_pairs.append_pair("git", git.git.as_str());
                if let Some(subdirectory) = &git.subdirectory {
                    query_pairs.append_pair("subdirectory", subdirectory);
                }
                match &git.rev {
                    Some(GitReference::Branch(branch)) => {
                        query_pairs.append_pair("branch", branch);
                    }
                    Some(GitReference::Rev(rev)) => {
                        query_pairs.append_pair("rev", rev);
                    }
                    Some(GitReference::Tag(tag)) => {
                        query_pairs.append_pair("tag", tag);
                    }
                    _ => {}
                }
            }
            pixi_build_types::SourcePackageLocationSpec::Path(path) => {
                query_pairs.append_pair("path", &path.path);
            }
        };
        drop(query_pairs);
        Self(url)
    }
}

#[cfg(test)]
mod test {
    use rattler_digest::{Md5, Sha256};

    use super::*;

    #[test]
    fn test_conversion() {
        let specs: Vec<SourcePackageSpec> = vec![
            pixi_build_types::PathSpec {
                path: "..\\test\\path".into(),
            }
            .into(),
            pixi_build_types::PathSpec {
                path: "../test/path".into(),
            }
            .into(),
            pixi_build_types::PathSpec {
                path: "test/path".into(),
            }
            .into(),
            pixi_build_types::PathSpec {
                path: "/absolute/test/path".into(),
            }
            .into(),
            pixi_build_types::PathSpec {
                path: "C://absolute/win/path".into(),
            }
            .into(),
            pixi_build_types::GitSpec {
                git: "https://github.com/some/repo.git".parse().unwrap(),
                rev: Some(GitReference::Rev("1234567890abcdef".into())),
                subdirectory: Some("subdir".into()),
            }
            .into(),
            pixi_build_types::UrlSpec {
                url: "https://example.com/some/file.tar.gz".parse().unwrap(),
                md5: Some(
                    rattler_digest::parse_digest_from_hex::<Md5>(
                        "d41d8cd98f00b204e9800998ecf8427e",
                    )
                    .unwrap(),
                ),
                sha256: Some(
                    rattler_digest::parse_digest_from_hex::<Sha256>(
                        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    )
                    .unwrap(),
                ),
                subdirectory: None,
            }
            .into(),
        ];

        for spec in specs {
            let url: EncodedSourceSpecUrl = spec.clone().into();
            let converted_spec: SourcePackageSpec = url.into();
            assert_eq!(spec, converted_spec);
        }
    }
}
