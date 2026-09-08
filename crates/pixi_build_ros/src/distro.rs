//! ROS distribution metadata.
//!
//! Maps a ROS distribution name to the ROS version it belongs to. That version
//! selects the distro mutex package, the `ROS_VERSION` build environment
//! variable, and whether `ros_workspace` is added as a build dependency.

/// The ROS 1 distributions.
///
/// ROS 1 reached end-of-life with noetic (EOSL May 2025), so this list is
/// closed and any distribution outside it is ROS 2.
///
/// Upstream `index-v4.yaml` only lists `groovy` onward; the earlier names are
/// included so the list covers every ROS 1 release.
const ROS1_DISTROS: &[&str] = &[
    "boxturtle",
    "cturtle",
    "diamondback",
    "electric",
    "fuerte",
    "groovy",
    "hydro",
    "indigo",
    "jade",
    "kinetic",
    "lunar",
    "melodic",
    "noetic",
];

/// The major ROS version a distribution belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosVersion {
    Ros1,
    Ros2,
}

impl RosVersion {
    /// The version the given distribution belongs to.
    fn for_distro(name: &str) -> Self {
        if ROS1_DISTROS.contains(&name) {
            Self::Ros1
        } else {
            Self::Ros2
        }
    }

    /// The mutex package that pins a build to a single ROS distribution.
    pub fn mutex_package_name(self) -> &'static str {
        match self {
            Self::Ros1 => "ros-distro-mutex",
            Self::Ros2 => "ros2-distro-mutex",
        }
    }

    /// The value of the `ROS_VERSION` environment variable.
    pub fn as_env_value(self) -> &'static str {
        match self {
            Self::Ros1 => "1",
            Self::Ros2 => "2",
        }
    }
}

/// A ROS distribution and the ROS version it belongs to.
#[derive(Debug, Clone)]
pub struct Distro {
    pub name: String,
    pub version: RosVersion,
}

impl Distro {
    /// The distribution with the given name.
    ///
    /// The name is lowercased and stripped of surrounding whitespace, matching
    /// the normalization conda applies to the package names derived from it.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into().trim().to_lowercase();
        let version = RosVersion::for_distro(&name);
        Self { name, version }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ros1_distro() {
        let distro = Distro::new("noetic");
        assert_eq!(distro.name, "noetic");
        assert_eq!(distro.version, RosVersion::Ros1);
    }

    #[test]
    fn test_ros2_distro() {
        assert_eq!(Distro::new("jazzy").version, RosVersion::Ros2);
        assert_eq!(Distro::new("rolling").version, RosVersion::Ros2);
    }

    #[test]
    fn test_every_ros1_distro_is_recognized() {
        for name in ROS1_DISTROS {
            assert_eq!(Distro::new(*name).version, RosVersion::Ros1, "{name}");
        }
    }

    /// Distributions released after this code was written are ROS 2, so they
    /// need no changes here.
    #[test]
    fn test_distro_postdating_this_code_is_ros2() {
        assert_eq!(Distro::new("lyrical").version, RosVersion::Ros2);
    }

    /// Conda lowercases the package names derived from the distro, so the
    /// classification has to agree with that rather than with what was typed.
    #[test]
    fn test_name_is_normalized() {
        let distro = Distro::new("  Noetic  ");
        assert_eq!(distro.name, "noetic");
        assert_eq!(distro.version, RosVersion::Ros1);

        assert_eq!(Distro::new("JAZZY").name, "jazzy");
    }

    #[test]
    fn test_unknown_distro_is_ros2() {
        assert_eq!(Distro::new("not-a-distro").version, RosVersion::Ros2);
    }

    #[test]
    fn test_mutex_package_name() {
        assert_eq!(RosVersion::Ros1.mutex_package_name(), "ros-distro-mutex");
        assert_eq!(RosVersion::Ros2.mutex_package_name(), "ros2-distro-mutex");
    }

    #[test]
    fn test_env_value() {
        assert_eq!(RosVersion::Ros1.as_env_value(), "1");
        assert_eq!(RosVersion::Ros2.as_env_value(), "2");
    }
}
