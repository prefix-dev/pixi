mod spanned;

use rattler_conda_types::ChannelConfig;
pub use spanned::PixiSpanned;
use url::Url;

// TODO(baszalmstra): Get rid of this ASAP
pub fn default_channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(
        std::env::current_dir().expect("Could not retrieve the current directory"),
    )
}

/// If the URL points to a subdirectory, extract it, as in (git):
///   `git+https://git.example.com/MyProject.git@v1.0#subdirectory=pkg_dir`
///   `git+https://git.example.com/MyProject.git@v1.0#egg=pkg&subdirectory=pkg_dir`
/// or (direct archive url):
///   `https://github.com/foo-labs/foo/archive/master.zip#subdirectory=packages/bar`
///   `https://github.com/foo-labs/foo/archive/master.zip#egg=pkg&subdirectory=packages/bar`
pub fn extract_directory_from_url(url: &Url) -> Option<String> {
    let fragment = url.fragment()?;
    let subdirectory = fragment
        .split('&')
        .find_map(|fragment| fragment.strip_prefix("subdirectory="))?;
    Some(subdirectory.into())
}
