//! Structured description of input globs a backend used while producing a
//! result.  This type mirrors `pixi_glob::GlobSet`'s constructor surface
//! so a backend can describe its inputs in a form pixi can replay
//! verbatim, including marker-driven workspace discovery and hidden-folder
//! handling that the flat `Vec<String>` form cannot express.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

/// One group of inputs the backend wants pixi to monitor.  When a result
/// carries multiple `InputGlobSet`s pixi walks each independently and unions
/// the resulting match sets, so different groups can carry mutually
/// incompatible includes/excludes/markers without interfering.
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InputGlobSet {
    /// Gitignore-style patterns: bare patterns include, `!`-prefixed
    /// patterns exclude.  Order matters under last-match-wins.
    pub patterns: Vec<String>,

    /// File names the walker probes for in each directory it enters.  A
    /// marker present in a directory dispatches against [`Self::patterns`]:
    /// match an include → record the marker path as a leaf and stop
    /// descent; match an exclude or nothing → prune the subtree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<String>,

    /// When `true` (the default), hidden directories (names starting with
    /// `.`) are skipped unless an include pattern opts them in explicitly.
    #[serde(default = "default_true")]
    pub exclude_hidden: bool,

    /// Walk root for this group.  `None` means the caller-supplied root
    /// (typically the package manifest directory at consumer call sites).
    /// When `Some`, an absolute path is used as-is and a relative path is
    /// joined onto the caller-supplied root, so a backend can target a
    /// workspace via `root: Some("../..")` or `root: Some(<workspace>)`
    /// without baking `../..` segments into every pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
}

impl Default for InputGlobSet {
    fn default() -> Self {
        Self {
            patterns: Vec::new(),
            markers: Vec::new(),
            exclude_hidden: true,
            root: None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_markers_and_hidden_default() {
        let json = r#"{"patterns": ["**/*.rs"]}"#;
        let parsed: InputGlobSet = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.patterns, vec!["**/*.rs".to_string()]);
        assert!(parsed.markers.is_empty());
        assert!(parsed.exclude_hidden);
        assert!(parsed.root.is_none());
    }

    #[test]
    fn full_round_trip() {
        let original = InputGlobSet {
            patterns: vec!["**/package.xml".to_string()],
            markers: vec!["package.xml".to_string(), "COLCON_IGNORE".to_string()],
            exclude_hidden: true,
            root: Some(PathBuf::from("../..")),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: InputGlobSet = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn empty_markers_and_none_root_are_skipped_when_serializing() {
        let v = InputGlobSet {
            patterns: vec!["**/*.cpp".to_string()],
            markers: Vec::new(),
            exclude_hidden: true,
            root: None,
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(
            !json.contains("markers"),
            "empty markers should be skipped, got: {json}"
        );
        assert!(
            !json.contains("root"),
            "absent root should be skipped, got: {json}"
        );
    }
}
