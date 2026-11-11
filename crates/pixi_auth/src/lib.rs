//! Authentication utilities for pixi.
//!
//! This crate provides a centralized way to create and configure authentication
//! storage that respects pixi's configuration settings.

use std::{any::Any, path::PathBuf, sync::Arc};

use pixi_config::Config;
use rattler_networking::{
    AuthenticationMiddleware, AuthenticationStorage,
    authentication_storage::{self, AuthenticationStorageError},
};

/// Creates an [`AuthenticationStorage`] configured according to pixi's settings.
///
/// This respects:
/// - Environment variables (`RATTLER_AUTH_FILE`)
/// - Pixi config's `authentication_override_file` setting
/// - Default system keyring/credential storage
///
/// # Example
///
/// ```no_run
/// use pixi_auth::get_auth_store;
/// use pixi_config::Config;
///
/// let config = Config::load_global();
/// let auth_storage = get_auth_store(&config).expect("Failed to create auth storage");
/// ```
pub fn get_auth_store(
    config: &Config,
) -> Result<AuthenticationStorage, AuthenticationStorageError> {
    let mut store = AuthenticationStorage::from_env_and_defaults()?;
    if let Some(auth_file) = config.authentication_override_file() {
        tracing::info!("Loading authentication from file: {:?}", auth_file);

        if !auth_file.exists() {
            tracing::warn!("Authentication file does not exist: {:?}", auth_file);
        }

        // This should be the first place before the keyring authentication
        // i.e. either index 0 if RATTLER_AUTH_FILE is not set or index 1 if it is
        let first_storage = store.backends.first().unwrap();
        let index = if first_storage.type_id()
            == std::any::TypeId::of::<authentication_storage::backends::file::FileStorage>()
        {
            1
        } else {
            0
        };
        store.backends.insert(
            index,
            Arc::from(
                authentication_storage::backends::file::FileStorage::from_path(PathBuf::from(
                    auth_file,
                ))?,
            ),
        );
    }
    Ok(store)
}

/// Creates an [`AuthenticationMiddleware`] configured according to pixi's settings.
///
/// This is a convenience function that creates an auth store and wraps it in middleware.
pub fn get_auth_middleware(
    config: &Config,
) -> Result<AuthenticationMiddleware, AuthenticationStorageError> {
    Ok(AuthenticationMiddleware::from_auth_storage(get_auth_store(
        config,
    )?))
}
