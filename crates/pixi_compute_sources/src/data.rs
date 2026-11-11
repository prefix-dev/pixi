//! Engine-side accessors for the source resolvers.

use pixi_compute_engine::DataStore;
use pixi_git::resolver::GitResolver;
use pixi_url::UrlResolver;

/// Access the git resolver from global data.
pub trait HasGitResolver {
    fn git_resolver(&self) -> &GitResolver;
}

impl HasGitResolver for DataStore {
    fn git_resolver(&self) -> &GitResolver {
        self.get::<GitResolver>()
    }
}

/// Access the URL resolver from global data.
pub trait HasUrlResolver {
    fn url_resolver(&self) -> &UrlResolver;
}

impl HasUrlResolver for DataStore {
    fn url_resolver(&self) -> &UrlResolver {
        self.get::<UrlResolver>()
    }
}
