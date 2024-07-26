mod spanned;

pub use spanned::PixiSpanned;
use url::Url;

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
pub(crate) fn default_channel_config() -> rattler_conda_types::ChannelConfig {
    rattler_conda_types::ChannelConfig::default_with_root_dir(
        std::env::current_dir().expect("Could not retrieve the current directory"),
    )
}
