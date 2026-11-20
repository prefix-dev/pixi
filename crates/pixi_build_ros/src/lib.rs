//! The ROS build backend's building blocks, also usable as a library (e.g. by
//! RoboStack's recipe generator): package.xml parsing with REP 149 condition
//! evaluation, package.xml licenses as SPDX, rosdep -> conda package mapping,
//! build script templates and workspace discovery.

pub mod build_script;
pub mod config;
pub mod distro;
pub mod license;
pub mod metadata;
pub mod package_map;
pub mod package_xml;
pub mod workspace_discovery;
