pub mod package_map;
mod spanned;

#[cfg(test)]
pub(crate) mod test_utils;
mod with_source_code;

pub use spanned::PixiSpanned;
use url::Url;
pub use with_source_code::WithSourceCode;

/// If the URL points to a subdirectory, extract it, as in (git):
///   `git+https://git.example.com/MyProject.git@v1.0#subdirectory=pkg_dir`
///   `git+https://git.example.com/MyProject.git@v1.0#egg=pkg&subdirectory=pkg_dir`
/// or (direct archive url):
///   `https://github.com/foo-labs/foo/archive/master.zip#subdirectory=packages/bar`
///   `https://github.com/foo-labs/foo/archive/master.zip#egg=pkg&subdirectory=packages/bar`
pub(crate) fn extract_directory_from_url(url: &Url) -> Option<String> {
    let fragment = url.fragment()?;
    let subdirectory = fragment
        .split('&')
        .find_map(|fragment| fragment.strip_prefix("subdirectory="))?;
    Some(subdirectory.into())
}

#[cfg(test)]
mod test {
    use rstest::*;
    use url::Url;

    use super::*;

    #[rstest]
    #[case(
        "git+https://github.com/foobar.git@v1.0#subdirectory=pkg_dir",
        Some("pkg_dir")
    )]
    #[case(
        "git+ssh://gitlab.org/foobar.git@v1.0#egg=pkg&subdirectory=pkg_dir",
        Some("pkg_dir")
    )]
    #[case("git+https://github.com/foobar.git@v1.0", None)]
    #[case("git+https://github.com/foobar.git@v1.0#egg=pkg", None)]
    #[case(
        "git+https://github.com/foobar.git@v1.0#subdirectory=pkg_dir&other=val",
        Some("pkg_dir")
    )]
    #[case(
        "git+https://github.com/foobar.git@v1.0#other=val&subdirectory=pkg_dir",
        Some("pkg_dir")
    )]
    #[case(
        "git+https://github.com/foobar.git@v1.0#subdirectory=pkg_dir&subdirectory=another_dir",
        Some("pkg_dir")
    )]
    fn test_get_subdirectory(#[case] url: Url, #[case] expected: Option<&str>) {
        let subdirectory = extract_directory_from_url(&url);
        assert_eq!(subdirectory.as_deref(), expected);
    }
}
