//! Library interface of the ROS build backend, so that other tools (e.g.
//! RoboStack's recipe generator) can reuse its package.xml parsing, condition
//! evaluation and rosdep -> conda mapping.

pub mod build_script;
pub mod config;
pub mod distro;
pub mod package_map;
pub mod package_xml;
pub mod workspace_discovery;
