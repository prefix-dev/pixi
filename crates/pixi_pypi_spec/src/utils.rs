use url::Url;

/// Parses URL fragment parameters into key-value pairs.
///
/// URL fragments like `#key1=value1&key2=value2` are parsed into a vector
/// of (key, value) tuples.
///
/// # Examples
/// - `#subdirectory=pkg_dir` -> `[("subdirectory", "pkg_dir")]`
/// - `#sha256=abc123&egg=foo` -> `[("sha256", "abc123"), ("egg", "foo")]`
pub fn parse_url_fragment_parameters(fragment: &str) -> Vec<(&str, &str)> {
    fragment
        .split('&')
        .filter_map(|param| param.split_once('='))
        .collect()
}

/// If the URL points to a subdirectory, extract it, as in (git):
///   `git+https://git.example.com/MyProject.git@v1.0#subdirectory=pkg_dir`
///   `git+https://git.example.com/MyProject.git@v1.0#egg=pkg&subdirectory=pkg_dir`
/// or (direct archive url):
///   `https://github.com/foo-labs/foo/archive/master.zip#subdirectory=packages/bar`
///   `https://github.com/foo-labs/foo/archive/master.zip#egg=pkg&subdirectory=packages/bar`
pub(crate) fn extract_directory_from_url(url: &Url) -> Option<String> {
    let fragment = url.fragment()?;
    parse_url_fragment_parameters(fragment)
        .into_iter()
        .find(|(key, _)| *key == "subdirectory")
        .map(|(_, value)| value.to_string())
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
