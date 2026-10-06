//! Parser for ROS `package.xml` manifest files.
//!
//! Uses `roxmltree` to parse the XML and extract package metadata,
//! dependencies, and build type information.
//! Supports package format 1, 2, and 3.

use std::collections::HashMap;

use miette::Diagnostic;
use thiserror::Error;

/// Errors that can occur when parsing a `package.xml` file.
#[derive(Debug, Error, Diagnostic)]
pub enum PackageXmlError {
    #[error("failed to parse package.xml XML")]
    #[diagnostic(help("Ensure the file is well-formed XML."))]
    XmlParse(#[source] roxmltree::Error),

    #[error("expected root element <package>, found <{found}>")]
    WrongRootElement { found: String },

    #[error("missing required <{element}> element in package.xml")]
    MissingElement { element: String },
}

/// A parsed `package.xml` file.
#[derive(Debug, Clone)]
pub struct PackageXml {
    pub format: u8,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub maintainers: Vec<Person>,
    pub licenses: Vec<String>,
    pub urls: Vec<UrlEntry>,
    pub dependencies: Dependencies,
    pub export: Export,
}

#[derive(Debug, Clone)]
pub struct Person {
    pub name: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UrlEntry {
    pub url: String,
    pub url_type: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Dependencies {
    pub build_depends: Vec<Dependency>,
    pub buildtool_depends: Vec<Dependency>,
    pub run_depends: Vec<Dependency>,
    pub exec_depends: Vec<Dependency>,
    pub depend: Vec<Dependency>,
    pub build_export_depends: Vec<Dependency>,
    pub buildtool_export_depends: Vec<Dependency>,
    pub test_depends: Vec<Dependency>,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub version_lt: Option<String>,
    pub version_lte: Option<String>,
    pub version_eq: Option<String>,
    pub version_gte: Option<String>,
    pub version_gt: Option<String>,
    pub condition: Option<String>,
    /// After condition evaluation: `true` if this dep should be included.
    /// `None` means no condition was set (always include).
    pub evaluated_condition: Option<bool>,
}

impl Dependency {
    /// Whether this dependency should be included (condition evaluated to true
    /// or no condition).
    pub fn is_active(&self) -> bool {
        self.evaluated_condition.unwrap_or(true)
    }
}

impl From<&str> for Dependency {
    fn from(name: &str) -> Self {
        Self {
            name: name.to_string(),
            version_lt: None,
            version_lte: None,
            version_eq: None,
            version_gte: None,
            version_gt: None,
            condition: None,
            evaluated_condition: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Export {
    pub build_type: Option<BuildType>,
    pub architecture_independent: bool,
    pub metapackage: bool,
}

#[derive(Debug, Clone)]
pub struct BuildType {
    pub name: String,
    pub condition: Option<String>,
}

impl PackageXml {
    /// Parse a `package.xml` string.
    pub fn parse(xml: &str) -> Result<Self, PackageXmlError> {
        let doc = roxmltree::Document::parse(xml).map_err(PackageXmlError::XmlParse)?;

        let root = doc.root_element();
        if root.tag_name().name() != "package" {
            return Err(PackageXmlError::WrongRootElement {
                found: root.tag_name().name().to_string(),
            });
        }

        let format: u8 = root
            .attribute("format")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        let name = get_element_text(&root, "name").ok_or(PackageXmlError::MissingElement {
            element: "name".to_string(),
        })?;
        let version =
            get_element_text(&root, "version").ok_or(PackageXmlError::MissingElement {
                element: "version".to_string(),
            })?;
        let description = get_element_text(&root, "description");

        let maintainers = root
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "maintainer")
            .map(|n| Person {
                name: n.text().map(|t| t.trim().to_string()).unwrap_or_default(),
                email: n.attribute("email").map(|s| s.to_string()),
            })
            .collect();

        let licenses: Vec<String> = root
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "license")
            .filter_map(|n| n.text().map(|t| t.trim().to_string()))
            .collect();

        let urls: Vec<UrlEntry> = root
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "url")
            .filter_map(|n| {
                n.text().map(|t| UrlEntry {
                    url: t.trim().to_string(),
                    url_type: n.attribute("type").map(|s| s.to_string()),
                })
            })
            .collect();

        let dependencies = Dependencies {
            build_depends: parse_deps(&root, "build_depend"),
            buildtool_depends: parse_deps(&root, "buildtool_depend"),
            run_depends: parse_deps(&root, "run_depend"),
            exec_depends: parse_deps(&root, "exec_depend"),
            depend: parse_deps(&root, "depend"),
            build_export_depends: parse_deps(&root, "build_export_depend"),
            buildtool_export_depends: parse_deps(&root, "buildtool_export_depend"),
            test_depends: parse_deps(&root, "test_depend"),
        };

        let export = parse_export(&root);

        Ok(PackageXml {
            format,
            name,
            version,
            description,
            maintainers,
            licenses,
            urls,
            dependencies,
            export,
        })
    }

    /// Get the build type from the export section, defaulting to "catkin".
    pub fn build_type(&self) -> String {
        self.export
            .build_type
            .as_ref()
            .map(|bt| bt.name.clone())
            .unwrap_or_else(|| "catkin".to_string())
    }

    /// Evaluate conditions on all dependencies using the provided environment
    /// variables.
    ///
    /// Returns a new `PackageXml` with `evaluated_condition` set on each
    /// dependency. Dependencies with conditions that evaluate to false will
    /// have `evaluated_condition = Some(false)`.
    pub fn evaluate_conditions(mut self, env: &HashMap<String, String>) -> Self {
        fn eval_deps(deps: &mut [Dependency], env: &HashMap<String, String>) {
            for dep in deps {
                if let Some(condition) = &dep.condition {
                    dep.evaluated_condition = Some(evaluate_condition(condition, env));
                }
            }
        }

        eval_deps(&mut self.dependencies.build_depends, env);
        eval_deps(&mut self.dependencies.buildtool_depends, env);
        eval_deps(&mut self.dependencies.run_depends, env);
        eval_deps(&mut self.dependencies.exec_depends, env);
        eval_deps(&mut self.dependencies.depend, env);
        eval_deps(&mut self.dependencies.build_export_depends, env);
        eval_deps(&mut self.dependencies.buildtool_export_depends, env);
        eval_deps(&mut self.dependencies.test_depends, env);

        self
    }
}

/// Extract the text content of a child element.
fn get_element_text(parent: &roxmltree::Node, tag: &str) -> Option<String> {
    parent
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == tag)
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
}

/// Parse dependency elements of a given tag name.
fn parse_deps(parent: &roxmltree::Node, tag: &str) -> Vec<Dependency> {
    parent
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == tag)
        .filter_map(|n| {
            let name = n.text().map(|t| t.trim().to_string())?;
            if name.is_empty() {
                return None;
            }
            Some(Dependency {
                name,
                version_lt: n.attribute("version_lt").map(|s| s.to_string()),
                version_lte: n.attribute("version_lte").map(|s| s.to_string()),
                version_eq: n.attribute("version_eq").map(|s| s.to_string()),
                version_gte: n.attribute("version_gte").map(|s| s.to_string()),
                version_gt: n.attribute("version_gt").map(|s| s.to_string()),
                condition: n.attribute("condition").map(|s| s.to_string()),
                evaluated_condition: None,
            })
        })
        .collect()
}

/// Parse the `<export>` section.
fn parse_export(parent: &roxmltree::Node) -> Export {
    let export_node = parent
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "export");

    let Some(export) = export_node else {
        return Export::default();
    };

    let build_type = export
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "build_type")
        .and_then(|n| {
            n.text().map(|t| BuildType {
                name: t.trim().to_string(),
                condition: n.attribute("condition").map(|s| s.to_string()),
            })
        });

    let architecture_independent = export
        .children()
        .any(|n| n.is_element() && n.tag_name().name() == "architecture_independent");

    let metapackage = export
        .children()
        .any(|n| n.is_element() && n.tag_name().name() == "metapackage");

    Export {
        build_type,
        architecture_independent,
        metapackage,
    }
}

/// Evaluate a condition expression as defined in REP 149, for example
/// `$ROS_VERSION == 2` or `$ROS_DISTRO != jazzy and $ROS_DISTRO != kilted`.
///
/// Supports comparison operators `==`, `!=`, `<`, `>`, `<=`, `>=`, combined
/// with `and` / `or` (where `and` binds tighter) and parentheses.
/// Variables are substituted from the `env` map using `$VAR` syntax.
/// Missing variables evaluate to empty string.
///
/// A condition that cannot be parsed is logged and evaluates to `true`, so the
/// dependency is kept rather than silently dropped.
fn evaluate_condition(condition: &str, env: &HashMap<String, String>) -> bool {
    let tokens = tokenize_condition(condition);
    let mut parser = ConditionParser {
        tokens: &tokens,
        pos: 0,
        env,
    };
    match parser.parse_or() {
        Some(result) if parser.pos == tokens.len() => result,
        _ => {
            tracing::warn!(
                "failed to parse package.xml condition '{condition}', including the dependency"
            );
            true
        }
    }
}

#[derive(Debug, PartialEq)]
enum ConditionToken {
    LParen,
    RParen,
    Comparison(&'static str),
    And,
    Or,
    Term(String),
}

/// Split a condition into tokens. Quoted strings are kept as a single term.
fn tokenize_condition(condition: &str) -> Vec<ConditionToken> {
    const COMPARISONS: [&str; 6] = ["==", "!=", "<=", ">=", "<", ">"];

    let mut tokens = Vec::new();
    let mut rest = condition.trim_start();
    while let Some(c) = rest.chars().next() {
        if c == '(' {
            tokens.push(ConditionToken::LParen);
            rest = &rest[1..];
        } else if c == ')' {
            tokens.push(ConditionToken::RParen);
            rest = &rest[1..];
        } else if let Some(op) = COMPARISONS.iter().find(|op| rest.starts_with(**op)) {
            tokens.push(ConditionToken::Comparison(op));
            rest = &rest[op.len()..];
        } else if c == '\'' || c == '"' {
            let end = rest[1..].find(c).map_or(rest.len(), |i| i + 2);
            tokens.push(ConditionToken::Term(rest[..end].to_string()));
            rest = &rest[end..];
        } else {
            let end = rest
                .find(|ch: char| ch.is_whitespace() || "()=!<>".contains(ch))
                .unwrap_or(rest.len())
                .max(1);
            let word = &rest[..end];
            tokens.push(match word {
                "and" => ConditionToken::And,
                "or" => ConditionToken::Or,
                _ => ConditionToken::Term(word.to_string()),
            });
            rest = &rest[end..];
        }
        rest = rest.trim_start();
    }
    tokens
}

/// Recursive descent parser that evaluates a tokenized condition.
struct ConditionParser<'a> {
    tokens: &'a [ConditionToken],
    pos: usize,
    env: &'a HashMap<String, String>,
}

impl ConditionParser<'_> {
    fn next_if(&mut self, expected: &ConditionToken) -> bool {
        if self.tokens.get(self.pos) == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// `or_expr := and_expr ('or' and_expr)*`
    fn parse_or(&mut self) -> Option<bool> {
        let mut result = self.parse_and()?;
        while self.next_if(&ConditionToken::Or) {
            // Evaluate the right-hand side unconditionally so parse errors surface.
            let rhs = self.parse_and()?;
            result = result || rhs;
        }
        Some(result)
    }

    /// `and_expr := atom ('and' atom)*`
    fn parse_and(&mut self) -> Option<bool> {
        let mut result = self.parse_atom()?;
        while self.next_if(&ConditionToken::And) {
            let rhs = self.parse_atom()?;
            result = result && rhs;
        }
        Some(result)
    }

    /// `atom := '(' or_expr ')' | term (comparison term)?`
    fn parse_atom(&mut self) -> Option<bool> {
        if self.next_if(&ConditionToken::LParen) {
            let result = self.parse_or()?;
            return self.next_if(&ConditionToken::RParen).then_some(result);
        }

        let lhs = self.parse_term()?;
        let Some(ConditionToken::Comparison(op)) = self.tokens.get(self.pos) else {
            // No operator; treat non-empty as true
            return Some(!lhs.is_empty());
        };
        self.pos += 1;
        let rhs = self.parse_term()?;

        Some(match *op {
            "==" => lhs == rhs,
            "!=" => lhs != rhs,
            "<" => lhs < rhs,
            ">" => lhs > rhs,
            "<=" => lhs <= rhs,
            ">=" => lhs >= rhs,
            _ => unreachable!("tokenizer only produces known comparison operators"),
        })
    }

    fn parse_term(&mut self) -> Option<String> {
        let ConditionToken::Term(term) = self.tokens.get(self.pos)? else {
            return None;
        };
        self.pos += 1;
        Some(substitute_vars(term, self.env))
    }
}

/// Substitute `$VAR` references in a string with values from the env map.
/// Also strips surrounding quotes (single or double).
fn substitute_vars(s: &str, env: &HashMap<String, String>) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            let mut var_name = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_alphanumeric() || next == '_' {
                    var_name.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if let Some(val) = env.get(&var_name) {
                result.push_str(val);
            }
        } else {
            result.push(c);
        }
    }

    // Strip surrounding quotes
    let trimmed = result.trim();
    if (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || (trimmed.starts_with('"') && trimmed.ends_with('"'))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_demo_nodes_cpp() {
        let xml = include_str!("../test_data/package_xmls/demo_nodes_cpp.xml");
        let pkg = PackageXml::parse(xml).unwrap();

        assert_eq!(pkg.name, "demo_nodes_cpp");
        assert_eq!(pkg.version, "0.37.1");
        assert_eq!(pkg.format, 2);
        assert_eq!(pkg.licenses, vec!["Apache License 2.0"]);
        assert_eq!(pkg.maintainers.len(), 2);
        assert_eq!(pkg.build_type(), "ament_cmake");

        let build_dep_names: Vec<&str> = pkg
            .dependencies
            .build_depends
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert!(build_dep_names.contains(&"rclcpp"));
        assert!(build_dep_names.contains(&"example_interfaces"));
    }

    #[test]
    fn test_parse_custom_ros() {
        let xml = include_str!("../test_data/package_xmls/custom_ros.xml");
        let pkg = PackageXml::parse(xml).unwrap();

        assert_eq!(pkg.name, "custom_ros");
        assert_eq!(pkg.version, "0.0.1");
        assert_eq!(pkg.format, 3);
        assert_eq!(pkg.description.as_deref(), Some("Demo"));
        assert_eq!(pkg.licenses, vec!["Apache License 2.0"]);

        let depend_names: Vec<&str> = pkg
            .dependencies
            .depend
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert!(depend_names.contains(&"ros_package"));
        assert!(depend_names.contains(&"multi_package"));
    }

    #[test]
    fn test_parse_broken_xml() {
        let xml = include_str!("../test_data/package_xmls/broken.xml");
        let result = PackageXml::parse(xml);
        assert!(matches!(result, Err(PackageXmlError::XmlParse(_))));
    }

    #[test]
    fn test_condition_evaluation() {
        let xml = r#"<?xml version="1.0"?>
<package format="3">
  <name>conditional_pkg</name>
  <version>0.1.0</version>
  <description>Test</description>
  <maintainer email="test@example.com">Tester</maintainer>
  <license>MIT</license>
  <buildtool_depend condition="$ROS_VERSION == 2">ament_cmake</buildtool_depend>
  <buildtool_depend condition="$ROS_VERSION == 1">catkin</buildtool_depend>
  <build_depend condition="$ROS_VERSION == 2">rclcpp</build_depend>
  <build_depend condition="$ROS_VERSION == 1">roscpp</build_depend>
  <exec_depend condition="$ROS_VERSION == 2">rclcpp</exec_depend>
  <exec_depend condition="$ROS_VERSION == 1">roscpp</exec_depend>
</package>"#;

        let pkg = PackageXml::parse(xml).unwrap();

        // ROS2 environment
        let env = HashMap::from([
            ("ROS_VERSION".to_string(), "2".to_string()),
            ("ROS_DISTRO".to_string(), "jazzy".to_string()),
        ]);
        let pkg_ros2 = pkg.clone().evaluate_conditions(&env);

        let active_buildtool: Vec<&str> = pkg_ros2
            .dependencies
            .buildtool_depends
            .iter()
            .filter(|d| d.is_active())
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(active_buildtool, vec!["ament_cmake"]);

        let active_build: Vec<&str> = pkg_ros2
            .dependencies
            .build_depends
            .iter()
            .filter(|d| d.is_active())
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(active_build, vec!["rclcpp"]);

        // ROS1 environment
        let env = HashMap::from([
            ("ROS_VERSION".to_string(), "1".to_string()),
            ("ROS_DISTRO".to_string(), "noetic".to_string()),
        ]);
        let pkg_ros1 = pkg.evaluate_conditions(&env);

        let active_buildtool: Vec<&str> = pkg_ros1
            .dependencies
            .buildtool_depends
            .iter()
            .filter(|d| d.is_active())
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(active_buildtool, vec!["catkin"]);
    }

    #[test]
    fn test_version_constraints_parsing() {
        let xml = include_str!("../test_data/package_xmls/version_constraints.xml");
        let pkg = PackageXml::parse(xml).unwrap();

        let deps = &pkg.dependencies.depend;

        let ros_pkg = deps.iter().find(|d| d.name == "ros_package").unwrap();
        assert_eq!(ros_pkg.version_lte.as_deref(), Some("2.0.0"));

        let qt = deps.iter().find(|d| d.name == "libqt5-core").unwrap();
        assert_eq!(qt.version_gte.as_deref(), Some("5.15.0"));
        assert_eq!(qt.version_lt.as_deref(), Some("5.16.0"));

        let tinyxml = deps.iter().find(|d| d.name == "tinyxml2").unwrap();
        assert_eq!(tinyxml.version_eq.as_deref(), Some("10.0.0"));
    }

    #[test]
    fn test_evaluate_condition_basic() {
        let env = HashMap::from([
            ("ROS_VERSION".to_string(), "2".to_string()),
            ("ROS_DISTRO".to_string(), "jazzy".to_string()),
        ]);

        assert!(evaluate_condition("$ROS_VERSION == 2", &env));
        assert!(!evaluate_condition("$ROS_VERSION == 1", &env));
        assert!(evaluate_condition("$ROS_VERSION != 1", &env));
        assert!(evaluate_condition("$ROS_DISTRO == jazzy", &env));
        assert!(!evaluate_condition("$ROS_DISTRO != 'jazzy'", &env));
    }

    #[test]
    fn test_evaluate_condition_and_or() {
        let env = HashMap::from([
            ("ROS_VERSION".to_string(), "2".to_string()),
            ("ROS_DISTRO".to_string(), "jazzy".to_string()),
        ]);

        // Regression test for https://github.com/prefix-dev/pixi/issues/7177
        assert!(!evaluate_condition(
            "$ROS_DISTRO != jazzy and $ROS_DISTRO != kilted",
            &env
        ));
        assert!(evaluate_condition(
            "$ROS_DISTRO != humble and $ROS_DISTRO != kilted",
            &env
        ));
        assert!(evaluate_condition(
            "$ROS_DISTRO == humble or $ROS_DISTRO == jazzy",
            &env
        ));
        assert!(!evaluate_condition(
            "$ROS_DISTRO == humble or $ROS_DISTRO == kilted",
            &env
        ));
    }

    #[test]
    fn test_evaluate_condition_precedence_and_parentheses() {
        let env = HashMap::from([
            ("ROS_VERSION".to_string(), "2".to_string()),
            ("ROS_DISTRO".to_string(), "jazzy".to_string()),
        ]);

        // `and` binds tighter than `or`.
        assert!(evaluate_condition(
            "$ROS_VERSION == 1 and $ROS_DISTRO == noetic or $ROS_DISTRO == jazzy",
            &env
        ));
        assert!(!evaluate_condition(
            "$ROS_VERSION == 1 and ($ROS_DISTRO == noetic or $ROS_DISTRO == jazzy)",
            &env
        ));
        assert!(evaluate_condition(
            "($ROS_VERSION == 2) and ($ROS_DISTRO == humble or $ROS_DISTRO == jazzy)",
            &env
        ));
    }

    #[test]
    fn test_evaluate_condition_without_spaces() {
        let env = HashMap::from([("ROS_VERSION".to_string(), "2".to_string())]);
        assert!(evaluate_condition("$ROS_VERSION==2", &env));
        assert!(evaluate_condition("($ROS_VERSION>=2)", &env));
    }

    #[test]
    fn test_evaluate_condition_malformed_keeps_dependency() {
        let env = HashMap::from([("ROS_VERSION".to_string(), "2".to_string())]);
        assert!(evaluate_condition("$ROS_VERSION == 1 and", &env));
        assert!(evaluate_condition("($ROS_VERSION == 1", &env));
    }

    #[test]
    fn test_substitute_vars() {
        let env = HashMap::from([("FOO".to_string(), "bar".to_string())]);
        assert_eq!(substitute_vars("$FOO", &env), "bar");
        assert_eq!(substitute_vars("'hello'", &env), "hello");
        assert_eq!(substitute_vars("$MISSING", &env), "");
    }
}
