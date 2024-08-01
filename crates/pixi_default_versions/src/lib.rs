use rattler_conda_types::{Platform, Version};

/// The default GLIBC version to use. This is used when no system requirements are specified.
pub fn default_glibc_version() -> Version {
    "2.28".parse().unwrap()
}

/// The default linux version to use. This is used when no system requirements are specified.
pub fn default_linux_version() -> Version {
    "5.10".parse().unwrap()
}

/// Returns the default Mac OS version for the specified platform. The platform must refer to a
/// MacOS platform.
pub fn default_mac_os_version(platform: Platform) -> Version {
    match platform {
        Platform::OsxArm64 => "13.0".parse().unwrap(),
        Platform::Osx64 => "13.0".parse().unwrap(),
        _ => panic!(
            "default_mac_os_version() called with non-osx platform: {}",
            platform
        ),
    }
}
