use std::{
    fmt::{Debug, Display},
    str::FromStr,
};

use indexmap::IndexMap;
use itertools::Itertools;
use rattler_conda_types::{PackageName, Platform, package::EntryPoint};
use serde::{Deserialize, Serialize};

use crate::{
    matchspec::{PackageDependency, SerializableMatchSpec},
    requirements::PackageSpecDependencies,
};

// Core enum for values that can be either concrete or templated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Value<T> {
    Concrete(T),
    Template(String), // Jinja template like "${{ name|lower }}"
}

impl<T: Display> Display for Value<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Concrete(val) => write!(f, "{val}"),
            Value::Template(template) => write!(f, "{template}"),
        }
    }
}

impl<T: ToString> Value<T> {
    pub fn concrete(&self) -> Option<&T> {
        if let Value::Concrete(val) = self {
            Some(val)
        } else {
            None
        }
    }
}

impl<T: ToString + FromStr> FromStr for Value<T>
where
    T::Err: std::fmt::Display,
{
    type Err = T::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains("${{") {
            // If it contains some template syntax, treat it as a template
            return Ok(Value::Template(s.to_string()));
        }

        Ok(Value::Concrete(T::from_str(s)?))
    }
}

impl From<SerializableMatchSpec> for Value<SerializableMatchSpec> {
    fn from(spec: SerializableMatchSpec) -> Self {
        Value::Concrete(spec)
    }
}

// Any item in a list can be either a value or a conditional
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Item<T> {
    Value(Value<T>),
    Conditional(Conditional<T>),
}

impl<T> Item<T> {
    pub fn new_from_conditional(condition: String, then: Vec<T>, else_value: Vec<T>) -> Self {
        Item::Conditional(Conditional {
            condition,
            then: ListOrItem::new(then),
            else_value: ListOrItem::new(else_value),
        })
    }
}

impl<T: Display> Display for Item<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Item::Value(value) => write!(f, "{value}"),
            Item::Conditional(cond) => write!(f, "{cond}"),
        }
    }
}

impl<T: PartialEq> PartialEq for Item<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Item::Value(Value::Concrete(a)), Item::Value(Value::Concrete(b))) => a == b,
            (Item::Conditional(a), Item::Conditional(b)) => {
                a.condition == b.condition && a.then == b.then && a.else_value == b.else_value
            }
            _ => false,
        }
    }
}

impl<T: Debug> Debug for Item<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Item::Value(value) => write!(f, "Value({value:?})"),
            Item::Conditional(cond) => write!(f, "Conditional({cond:?})"),
        }
    }
}

impl<T> From<Conditional<T>> for Item<T> {
    fn from(value: Conditional<T>) -> Self {
        Self::Conditional(value)
    }
}

impl From<Source> for Item<Source> {
    fn from(source: Source) -> Self {
        Item::Value(Value::Concrete(source))
    }
}

impl From<SerializableMatchSpec> for Item<SerializableMatchSpec> {
    fn from(matchspec: SerializableMatchSpec) -> Self {
        Item::Value(Value::Concrete(matchspec))
    }
}

impl From<PackageDependency> for Item<PackageDependency> {
    fn from(dep: PackageDependency) -> Self {
        Item::Value(Value::Concrete(dep))
    }
}

impl<T: ToString + FromStr> FromStr for Item<T>
where
    T::Err: std::fmt::Display,
{
    type Err = T::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains("${{") {
            // If it contains some template syntax, treat it as a template
            return Ok(Item::Value(Value::Template(s.to_string())));
        }

        let value = Value::Concrete(T::from_str(s)?);
        Ok(Item::Value(value))
    }
}
#[derive(Clone)]
pub struct ListOrItem<T>(pub Vec<T>);

impl<T> Default for ListOrItem<T> {
    fn default() -> Self {
        ListOrItem(Vec::new())
    }
}

impl<T: PartialEq> PartialEq for ListOrItem<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Debug> Debug for ListOrItem<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            write!(f, "ListOrItem([])")
        } else if self.0.len() == 1 {
            write!(f, "ListOrItem({:?})", self.0[0])
        } else {
            write!(f, "ListOrItem({:?})", self.0)
        }
    }
}

impl<T: FromStr> FromStr for ListOrItem<T> {
    type Err = T::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(ListOrItem::single(s.parse()?))
    }
}

impl<T> serde::Serialize for ListOrItem<T>
where
    T: serde::Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self.0.len() {
            1 => self.0[0].serialize(serializer),
            _ => self.0.serialize(serializer),
        }
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for ListOrItem<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use std::fmt;

        use serde::de::{Error, Visitor};

        struct ListOrItemVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: serde::Deserialize<'de>> Visitor<'de> for ListOrItemVisitor<T> {
            type Value = ListOrItem<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a single item or a list of items")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(item) = seq.next_element()? {
                    vec.push(item);
                }
                Ok(ListOrItem(vec))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                let item = T::deserialize(serde::de::value::StrDeserializer::new(value))?;
                Ok(ListOrItem(vec![item]))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: Error,
            {
                let item = T::deserialize(serde::de::value::StringDeserializer::new(value))?;
                Ok(ListOrItem(vec![item]))
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let item = T::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(ListOrItem(vec![item]))
            }
        }

        deserializer.deserialize_any(ListOrItemVisitor(std::marker::PhantomData))
    }
}

impl<T: ToString> Display for ListOrItem<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.len() {
            0 => write!(f, "[]"),
            1 => write!(f, "{}", self.0[0].to_string()),
            _ => write!(
                f,
                "[{}]",
                self.0
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl<T> ListOrItem<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self(items)
    }

    pub fn single(item: T) -> Self {
        Self(vec![item])
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }
}

// Conditional structure for if-else logic
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Conditional<T> {
    #[serde(rename = "if")]
    pub condition: String,
    pub then: ListOrItem<T>,
    #[serde(rename = "else")]
    pub else_value: ListOrItem<T>,
}

impl<T: Debug> Debug for Conditional<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Conditional {{ condition: {}, then: {:?}, else: {:?} }}",
            self.condition, self.then, self.else_value
        )
    }
}

impl<T: Display> Display for Conditional<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "if {} then {} else {}",
            self.condition, self.then, self.else_value
        )
    }
}

/// Type alias for lists that can contain conditionals
pub type ConditionalList<T> = Vec<Item<T>>;

// Main recipe structure
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct IntermediateRecipe {
    #[serde(default)]
    pub context: IndexMap<String, Value<String>>,
    #[serde(default)]
    pub package: Package,
    #[serde(default)]
    pub source: ConditionalList<Source>,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub requirements: ConditionalRequirements,
    #[serde(default)]
    pub tests: Vec<Test>,
    #[serde(default)]
    pub about: Option<About>,
    #[serde(default)]
    pub extra: Option<Extra>,
}

pub struct EvaluatedDependencies {
    pub build: Option<Vec<SerializableMatchSpec>>,
    pub host: Option<Vec<SerializableMatchSpec>>,
    pub run: Option<Vec<SerializableMatchSpec>>,
    pub run_constraints: Option<Vec<SerializableMatchSpec>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Package {
    pub name: Value<String>,
    pub version: Value<String>,
}

impl Default for Package {
    fn default() -> Self {
        Package {
            name: Value::Concrete("default-package".to_string()),
            version: Value::Concrete("0.0.1".to_string()),
        }
    }
}

impl Display for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.name, self.version)
    }
}

/// Source information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Source {
    /// Url source pointing to a tarball or similar to retrieve the source from
    Url(UrlSource),
    /// Path source pointing to a local path where the source can be found
    Path(PathSource),
}

impl Source {
    pub fn url(url: String) -> Self {
        Source::Url(UrlSource {
            url: Value::Concrete(url),
            sha256: None,
        })
    }

    pub fn path(path: String) -> Self {
        Source::Path(PathSource {
            path: Value::Concrete(path),
            sha256: None,
        })
    }

    pub fn with_sha256(self, sha256: String) -> Self {
        match self {
            Source::Url(mut url_source) => {
                url_source.sha256 = Some(Value::Concrete(sha256));
                Source::Url(url_source)
            }
            Source::Path(mut path_source) => {
                path_source.sha256 = Some(Value::Concrete(sha256));
                Source::Path(path_source)
            }
        }
    }
}

impl From<UrlSource> for Source {
    fn from(url_source: UrlSource) -> Self {
        Source::Url(url_source)
    }
}
impl From<PathSource> for Source {
    fn from(path_source: PathSource) -> Self {
        Source::Path(path_source)
    }
}

impl FromStr for Source {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("http://") || s.starts_with("https://") {
            Ok(Source::Url(UrlSource {
                url: Value::Concrete(s.to_string()),
                sha256: None,
            }))
        } else {
            Ok(Source::Path(PathSource {
                path: Value::Concrete(s.to_string()),
                sha256: None,
            }))
        }
    }
}

impl Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Url(url_source) => {
                let sha256 = url_source
                    .sha256
                    .as_ref()
                    .map_or("".to_string(), |s| s.to_string());
                write!(f, "url: {}, sha256: {}", url_source.url, sha256)
            }
            Source::Path(path_source) => {
                let sha256 = path_source
                    .sha256
                    .as_ref()
                    .map_or("".to_string(), |s| s.to_string());
                write!(f, "path: {}, sha256: {}", path_source.path, sha256)
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct UrlSource {
    pub url: Value<String>,
    pub sha256: Option<Value<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PathSource {
    pub path: Value<String>,
    pub sha256: Option<Value<String>>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Script {
    pub content: String,
    #[serde(default)]
    pub env: IndexMap<String, String>,
    #[serde(default)]
    pub secrets: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum NoArchKind {
    Python,
    Generic,
}

impl Display for NoArchKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoArchKind::Python => write!(f, "python"),
            NoArchKind::Generic => write!(f, "generic"),
        }
    }
}
/// Python specific build configuration
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Python {
    /// For a Python noarch package to have executables it is necessary to
    /// specify the python entry points. These contain the name of the
    /// executable and the module + function that should be executed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entry_points: Vec<EntryPoint>,
}

impl Python {
    /// Returns true if this is the default python configuration.
    pub fn is_default(&self) -> bool {
        self.entry_points.is_empty()
    }
}

impl Display for Python {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for entry_point in &self.entry_points {
            write!(f, "{entry_point}, ")?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Build {
    pub number: Option<Value<u64>>,
    pub script: Script,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noarch: Option<NoArchKind>,
    #[serde(default, skip_serializing_if = "Python::is_default")]
    pub python: Python,
}

impl Build {
    pub fn new(content: String) -> Self {
        Build {
            number: None,
            script: Script {
                content,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// A struct to hold the fully resolved, non-conditional requirements.
#[derive(Default)]
pub struct ResolvedRequirements {
    pub build: Vec<PackageDependency>,
    pub host: Vec<PackageDependency>,
    pub run: Vec<PackageDependency>,
    pub run_constraints: Vec<PackageDependency>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub enum Target {
    Default,
    Specific(String),
}

/// A type that is very specific to rattler-build /recipe.yaml side
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct ConditionalRequirements {
    #[serde(default)]
    pub build: ConditionalList<PackageDependency>,
    #[serde(default)]
    pub host: ConditionalList<PackageDependency>,
    #[serde(default)]
    pub run: ConditionalList<PackageDependency>,
    #[serde(default)]
    pub run_constraints: ConditionalList<PackageDependency>,
}

impl ConditionalRequirements {
    /// Resolves the conditional requirements for a given platform.
    pub fn resolve(
        build: &ConditionalList<PackageDependency>,
        host: &ConditionalList<PackageDependency>,
        run: &ConditionalList<PackageDependency>,
        run_constraints: &ConditionalList<PackageDependency>,
        platform: Option<Platform>,
    ) -> PackageSpecDependencies<PackageDependency> {
        PackageSpecDependencies {
            build: Self::resolve_list(build, platform),
            host: Self::resolve_list(host, platform),
            run: Self::resolve_list(run, platform),
            run_constraints: Self::resolve_list(run_constraints, platform),
        }
    }

    pub(crate) fn resolve_list(
        list: &ConditionalList<PackageDependency>,
        platform: Option<Platform>,
    ) -> IndexMap<PackageName, PackageDependency> {
        list.iter()
            .flat_map(|item| Self::resolve_item(item, platform))
            .collect()
    }

    pub(crate) fn resolve_item(
        item: &Item<PackageDependency>,
        platform: Option<Platform>,
    ) -> IndexMap<PackageName, PackageDependency> {
        match item {
            Item::Value(v) => {
                // Should we handle jinja here?
                if let Some(dep) = v.concrete() {
                    IndexMap::from([(dep.package_name(), dep.clone())])
                } else {
                    IndexMap::new()
                }
            }

            Item::Conditional(cond) => {
                if let Some(p) = platform {
                    // This is a simple string comparison
                    let dependencies = if cond.condition == *p.as_str() {
                        cond.then.clone().0.to_vec()
                    } else {
                        cond.else_value.clone().0.to_vec()
                    };

                    let mut map: IndexMap<PackageName, PackageDependency> = IndexMap::new();
                    for dep in dependencies {
                        map.insert(dep.package_name(), dep.clone());
                    }

                    map
                } else {
                    // If no platform is specified, conditional blocks are ignored.
                    IndexMap::new()
                }
            }
        }
    }
}

impl Display for ConditionalRequirements {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ build: {}, host: {}, run: {}, run_constraints: {} }}",
            self.build.iter().format(", "),
            self.host.iter().format(", "),
            self.run.iter().format(", "),
            self.run_constraints.iter().format(", "),
        )
    }
}

// #[derive(Debug, Serialize, Deserialize)]
// pub(crate) struct Requirements {
//     pub build: Vec<SerializableMatchSpec>,
//     pub host: Vec<SerializableMatchSpec>,
//     pub run: Vec<SerializableMatchSpec>,
//     pub run_constraints: Vec<SerializableMatchSpec>,
// }

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Test {
    pub package_contents: Option<PackageContents>,
}

impl Display for Test {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Test {{ package_contents: {} }}",
            self.package_contents.as_ref().into_iter().format("")
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PackageContents {
    pub include: Option<ConditionalList<String>>,
    pub files: Option<ConditionalList<String>>,
}

impl Display for PackageContents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PackageContents {{ include: {}, files: {} }}",
            self.include.as_ref().into_iter().flatten().format(", "),
            self.files.as_ref().into_iter().flatten().format(", "),
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct About {
    pub homepage: Option<Value<String>>,
    pub license: Option<Value<String>>,
    pub license_file: Option<Value<String>>,
    pub summary: Option<Value<String>>,
    pub description: Option<Value<String>>,
    pub documentation: Option<Value<String>>,
    pub repository: Option<Value<String>>,
}

impl Display for About {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "About {{ homepage: {}, license: {}, license_file: {}, summary: {}, description: {}, documentation: {}, repository: {} }}",
            self.homepage.as_ref().into_iter().format(", "),
            self.license.as_ref().into_iter().format(", "),
            self.license_file.as_ref().into_iter().format(", "),
            self.summary.as_ref().into_iter().format(", "),
            self.description.as_ref().into_iter().format(", "),
            self.documentation.as_ref().into_iter().format(", "),
            self.repository.as_ref().into_iter().format(", ")
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Extra {
    #[serde(rename = "recipe-maintainers")]
    pub recipe_maintainers: ConditionalList<String>,
}

impl Display for Extra {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ recipe_maintainers: {} }}",
            self.recipe_maintainers.iter().format(", ")
        )
    }
}

// Implementation for Recipe
impl IntermediateRecipe {
    /// Converts the recipe to YAML string
    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }

    /// Converts the recipe to pretty-formatted YAML string
    pub fn to_yaml_pretty(&self) -> Result<String, serde_yaml::Error> {
        // serde_yaml doesn't have a "pretty" option like serde_json,
        // but it produces readable YAML by default
        self.to_yaml()
    }

    /// Creates a recipe from YAML string
    pub fn from_yaml(yaml: &str) -> Result<IntermediateRecipe, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }
}

impl<T: ToString + Default + Debug> Conditional<T> {
    pub fn new(condition: String, then_value: ListOrItem<T>) -> Self {
        Self {
            condition,
            then: then_value,
            else_value: ListOrItem::default(),
        }
    }

    pub fn with_else(mut self, else_value: ListOrItem<T>) -> Self {
        self.else_value = else_value;
        self
    }
}

impl<T: ToString> Value<T> {
    pub fn is_template(&self) -> bool {
        matches!(self, Value::Template(_))
    }

    pub fn is_concrete(&self) -> bool {
        matches!(self, Value::Concrete(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recipe_to_yaml() {
        // Create a simple recipe
        let mut context = IndexMap::new();
        context.insert("name".to_string(), Value::Concrete("xtensor".to_string()));
        context.insert("version".to_string(), Value::Concrete("0.24.6".to_string()));

        let source = ConditionalList::from(vec![<Source as Into<Item<Source>>>::into(
            UrlSource {
                url: "https://github.com/xtensor-stack/xtensor/archive/${{ version }}.tar.gz"
                    .parse()
                    .unwrap(),
                sha256: Some(
                    "f87259b51aabafdd1183947747edfff4cff75d55375334f2e81cee6dc68ef655"
                        .parse()
                        .unwrap(),
                ),
            }
            .into(),
        )]);

        let recipe = IntermediateRecipe {
            context,
            package: Package {
                name: Value::Template("${{ name|lower }}".to_string()),
                version: Value::Template("${{ version }}".to_string()),
            },
            source,
            build: Build::default(),
            requirements: ConditionalRequirements {
                build: vec![
                    "${{ compiler('cxx') }}".parse().unwrap(),
                    "cmake".parse().unwrap(),
                    Conditional {
                        condition: "unix".to_owned(),
                        then: "make".parse().unwrap(),
                        else_value: "ninja".parse().unwrap(),
                    }
                    .into(),
                ],
                host: vec![
                    "xtl >=0.7,<0.8".parse().unwrap(),
                    "${{ context.name }}".parse().unwrap(),
                ],
                run: vec!["xtl >=0.7,<0.8".parse().unwrap()],
                run_constraints: vec!["xsimd >=8.0.3,<10".parse().unwrap()],
            },
            about: Some(About {
                homepage: Some(Value::Concrete(
                    "https://github.com/xtensor-stack/xtensor".to_string(),
                )),
                license: Some("BSD-3-Clause".parse().unwrap()),
                license_file: Some("LICENSE".parse().unwrap()),
                summary: Some("The C++ tensor algebra library".parse().unwrap()),
                description: Some(
                    "Multi dimensional arrays with broadcasting and lazy computing"
                        .parse()
                        .unwrap(),
                ),
                documentation: Some("https://xtensor.readthedocs.io".parse().unwrap()),
                repository: Some("https://github.com/xtensor-stack/xtensor".parse().unwrap()),
            }),
            extra: Some(Extra {
                recipe_maintainers: vec!["some-maintainer".parse().unwrap()],
            }),
            ..Default::default()
        };

        insta::assert_yaml_snapshot!(recipe)
    }
}
