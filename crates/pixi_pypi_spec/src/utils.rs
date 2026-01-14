use pixi_spec::Subdirectory;
use url::Url;

/// If the URL points to a subdirectory, extract it, as in (git):
///   `git+https://git.example.com/MyProject.git@v1.0#subdirectory=pkg_dir`
///   `git+https://git.example.com/MyProject.git@v1.0#egg=pkg&subdirectory=pkg_dir`
/// or (direct archive url):
///   `https://github.com/foo-labs/foo/archive/master.zip#subdirectory=packages/bar`
///   `https://github.com/foo-labs/foo/archive/master.zip#egg=pkg&subdirectory=packages/bar`
pub(crate) fn extract_directory_from_url(url: &Url) -> Subdirectory {
    let Some(fragment) = url.fragment() else {
        return Subdirectory::default();
    };
    let Some(subdirectory) = fragment
        .split('&')
        .find_map(|fragment| fragment.strip_prefix("subdirectory="))
    else {
        return Subdirectory::default();
    };
    Subdirectory::try_from(subdirectory).unwrap_or_default()
}

#[cfg(test)]
mod test {
    use rstest::*;
    use url::Url;

    use super::*;

    #[rstest]
    #[case(
        "git+https://github.com/foobar.git@v1.0#subdirectory=pkg_dir",
        "pkg_dir"
    )]
    #[case(
        "git+ssh://gitlab.org/foobar.git@v1.0#egg=pkg&subdirectory=pkg_dir",
        "pkg_dir"
    )]
    #[case("git+https://github.com/foobar.git@v1.0", "")]
    #[case("git+https://github.com/foobar.git@v1.0#egg=pkg", "")]
    #[case(
        "git+https://github.com/foobar.git@v1.0#subdirectory=pkg_dir&other=val",
        "pkg_dir"
    )]
    #[case(
        "git+https://github.com/foobar.git@v1.0#other=val&subdirectory=pkg_dir",
        "pkg_dir"
    )]
    #[case(
        "git+https://github.com/foobar.git@v1.0#subdirectory=pkg_dir&subdirectory=another_dir",
        "pkg_dir"
    )]
    fn test_get_subdirectory(#[case] url: Url, #[case] expected: &str) {
        let subdirectory = extract_directory_from_url(&url);
        assert_eq!(subdirectory.to_string(), expected);
    }
}
