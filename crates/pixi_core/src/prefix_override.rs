use rattler_conda_types::Platform;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

thread_local! {
    static PREFIX_OVERRIDES: RefCell<HashMap<String, PathBuf>> = RefCell::new(HashMap::new());
    static PLATFORM_OVERRIDES: RefCell<HashMap<String, Platform>> = RefCell::new(HashMap::new());
}

/// A RAII guard that manages thread-local prefix overrides
pub struct PrefixOverrideGuard {
    env_names: Vec<String>,
}

impl PrefixOverrideGuard {
    /// Create a new prefix override guard for a single environment
    pub fn new(env_name: String, custom_prefix: PathBuf) -> Self {
        PREFIX_OVERRIDES.with(|overrides| {
            overrides
                .borrow_mut()
                .insert(env_name.clone(), custom_prefix);
        });

        Self {
            env_names: vec![env_name],
        }
    }

    /// Create a new prefix override guard with platform override for a single environment
    pub fn new_with_platform(env_name: String, custom_prefix: PathBuf, platform: Platform) -> Self {
        PREFIX_OVERRIDES.with(|overrides| {
            overrides
                .borrow_mut()
                .insert(env_name.clone(), custom_prefix);
        });

        PLATFORM_OVERRIDES.with(|overrides| {
            overrides.borrow_mut().insert(env_name.clone(), platform);
        });

        Self {
            env_names: vec![env_name],
        }
    }
}

impl Drop for PrefixOverrideGuard {
    fn drop(&mut self) {
        // Remove all overrides for the environments this guard was managing
        PREFIX_OVERRIDES.with(|overrides| {
            let mut map = overrides.borrow_mut();
            for env_name in &self.env_names {
                map.remove(env_name);
            }
        });

        PLATFORM_OVERRIDES.with(|overrides| {
            let mut map = overrides.borrow_mut();
            for env_name in &self.env_names {
                map.remove(env_name);
            }
        });
    }
}

/// Get the prefix override for a specific environment, if any
pub fn get_prefix_override(env_name: &str) -> Option<PathBuf> {
    PREFIX_OVERRIDES.with(|overrides| overrides.borrow().get(env_name).cloned())
}

/// Get the platform override for a specific environment, if any
pub fn get_platform_override(env_name: &str) -> Option<Platform> {
    PLATFORM_OVERRIDES.with(|overrides| overrides.borrow().get(env_name).cloned())
}
