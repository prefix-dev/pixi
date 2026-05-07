//! Shared input-glob lists. Both the package.xml and pixi-native code paths
//! reference these so changes to "what counts as a source change" stay in
//! lockstep.

/// Globs that should always invalidate the build cache for any ROS package.
pub(crate) const ROS_SOURCE_GLOBS: &[&str] = &[
    "**/*.c",
    "**/*.cpp",
    "**/*.h",
    "**/*.hpp",
    "**/*.rs",
    "**/*.sh",
    "package.xml",
    "setup.py",
    "setup.cfg",
    "pyproject.toml",
    "Makefile",
    "CMakeLists.txt",
    "MANIFEST.in",
    "Cargo.toml",
    "Cargo.lock",
    "tests/**/*.py",
    "docs/**/*.rst",
    "docs/**/*.md",
    "launch/**/*.py",
    "config/*.yaml",
    "msg/**/*.msg",
    "srv/**/*.srv",
    "action/**/*.action",
];

/// Python source globs. Excluded by the package.xml flow when `editable = true`
/// (editable installs pick up Python source changes live without a rebuild).
/// Always included by the pixi-native flow.
pub(crate) const ROS_PYTHON_SOURCE_GLOBS: &[&str] = &["**/*.py", "**/*.pyx"];
