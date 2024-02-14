use anyhow::anyhow;
use rattler_networking::authentication_storage::backends::file::FileStorage;
use rattler_networking::authentication_storage::backends::keyring::KeyringAuthenticationStorage;
use rattler_networking::authentication_storage::backends::netrc::NetRcStorage;
use rattler_networking::authentication_storage::StorageBackend;
use rattler_networking::{Authentication, AuthenticationMiddleware, AuthenticationStorage};
use reqwest_middleware::ClientWithMiddleware;
use std::sync::Arc;

fn make_auth_storage() -> AuthenticationStorage {
    let mut storage = AuthenticationStorage::new();

    storage.add_backend(Arc::from(AuthHelperAuthBackend::default()));
    storage.add_backend(Arc::from(KeyringAuthenticationStorage::default()));
    storage.add_backend(Arc::from(FileStorage::default()));
    storage.add_backend(Arc::from(
        NetRcStorage::from_env().unwrap_or_else(|_| NetRcStorage::default()),
    ));

    storage
}

pub fn make_auth_client() -> ClientWithMiddleware {
    let auth_storage = make_auth_storage();
    let auth_middleware = AuthenticationMiddleware::new(auth_storage);

    let client = reqwest::Client::new();
    reqwest_middleware::ClientBuilder::new(client)
        .with(auth_middleware)
        .build()
}

#[derive(Debug, Default)]
struct AuthHelperAuthBackend {}

impl StorageBackend for AuthHelperAuthBackend {
    fn store(&self, _host: &str, _authentication: &Authentication) -> anyhow::Result<()> {
        Err(anyhow!("Can't store credentials in auth helpers"))
    }

    fn get(&self, _host: &str) -> anyhow::Result<Option<Authentication>> {
        Ok(None)
    }

    fn delete(&self, _: &str) -> anyhow::Result<()> {
        Err(anyhow!("Can't remove credentials from auth helpers"))
    }
}
