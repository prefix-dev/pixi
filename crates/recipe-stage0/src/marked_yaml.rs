use hashlink::LinkedHashMap;
use marked_yaml::types::{MarkedMappingNode, MarkedScalarNode, MarkedSequenceNode};
use marked_yaml::{Node as MarkedNode, Span};

pub type MappingHash = LinkedHashMap<MarkedScalarNode, MarkedNode>;

use crate::recipe::{
    About, Build, Conditional, ConditionalList, ConditionalRequirements, Extra, IntermediateRecipe,
    Item, ListOrItem, Package, PackageContents, Source, Test, Value,
};

// Trait for converting to marked YAML nodes
pub trait ToMarkedYaml {
    fn to_marked_yaml(&self) -> MarkedNode;
}

impl<T> ToMarkedYaml for Value<T>
where
    T: ToString,
{
    fn to_marked_yaml(&self) -> MarkedNode {
        let value_str = match self {
            Value::Concrete(val) => val.to_string(),
            Value::Template(template) => template.clone(),
        };
        MarkedNode::Scalar(MarkedScalarNode::new(Span::new_blank(), value_str))
    }
}

impl<T> ToMarkedYaml for Item<T>
where
    T: ToString,
{
    fn to_marked_yaml(&self) -> MarkedNode {
        match self {
            Item::Value(value) => value.to_marked_yaml(),
            Item::Conditional(conditional) => conditional.to_marked_yaml(),
        }
    }
}

impl<T: ToString> ToMarkedYaml for Conditional<T> {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        // Add the "if" condition
        mapping.insert(
            MarkedScalarNode::new(Span::new_blank(), "if"),
            MarkedNode::Scalar(MarkedScalarNode::new(Span::new_blank(), &self.condition)),
        );

        // Add the "then" value
        mapping.insert(
            MarkedScalarNode::new(Span::new_blank(), "then"),
            self.then.to_marked_yaml(),
        );

        // Add the "else" value if present
        if !self.else_value.0.is_empty() {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "else"),
                self.else_value.to_marked_yaml(),
            );
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl<T> ToMarkedYaml for ConditionalList<T>
where
    T: ToString,
{
    fn to_marked_yaml(&self) -> MarkedNode {
        let nodes: Vec<MarkedNode> = self.iter().map(|item| item.to_marked_yaml()).collect();
        MarkedNode::Sequence(MarkedSequenceNode::new(Span::new_blank(), nodes))
    }
}

impl ToMarkedYaml for Package {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        mapping.insert(
            MarkedScalarNode::new(Span::new_blank(), "name"),
            self.name.to_marked_yaml(),
        );
        mapping.insert(
            MarkedScalarNode::new(Span::new_blank(), "version"),
            self.version.to_marked_yaml(),
        );

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl ToMarkedYaml for Source {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        match self {
            Source::Path(path) => {
                mapping.insert(
                    MarkedScalarNode::new(Span::new_blank(), "path"),
                    path.path.to_marked_yaml(),
                );
                if let Some(ref sha256) = path.sha256 {
                    mapping.insert(
                        MarkedScalarNode::new(Span::new_blank(), "sha256"),
                        sha256.to_marked_yaml(),
                    );
                }
            }
            Source::Url(url) => {
                mapping.insert(
                    MarkedScalarNode::new(Span::new_blank(), "url"),
                    url.url.to_marked_yaml(),
                );
                if let Some(ref sha256) = url.sha256 {
                    mapping.insert(
                        MarkedScalarNode::new(Span::new_blank(), "sha256"),
                        sha256.to_marked_yaml(),
                    );
                }
            }
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl ToMarkedYaml for Build {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        if let Some(ref number) = self.number {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "number"),
                number.to_marked_yaml(),
            );
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl ToMarkedYaml for ConditionalRequirements {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        if !self.build.is_empty() {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "build"),
                self.build.to_marked_yaml(),
            );
        }

        if !self.host.is_empty() {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "host"),
                self.host.to_marked_yaml(),
            );
        }

        if !self.run.is_empty() {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "run"),
                self.run.to_marked_yaml(),
            );
        }

        if !self.run_constraints.is_empty() {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "run_constraints"),
                self.run_constraints.to_marked_yaml(),
            );
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl ToMarkedYaml for PackageContents {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        if let Some(ref include) = self.include {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "include"),
                include.to_marked_yaml(),
            );
        }

        if let Some(ref files) = self.files {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "files"),
                files.to_marked_yaml(),
            );
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl ToMarkedYaml for Test {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        if let Some(ref package_contents) = self.package_contents {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "package_contents"),
                package_contents.to_marked_yaml(),
            );
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl ToMarkedYaml for About {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        if let Some(ref homepage) = self.homepage {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "homepage"),
                homepage.to_marked_yaml(),
            );
        }

        if let Some(ref license) = self.license {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "license"),
                license.to_marked_yaml(),
            );
        }

        if let Some(ref license_file) = self.license_file {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "license_file"),
                license_file.to_marked_yaml(),
            );
        }

        if let Some(ref summary) = self.summary {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "summary"),
                summary.to_marked_yaml(),
            );
        }

        if let Some(ref description) = self.description {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "description"),
                description.to_marked_yaml(),
            );
        }

        if let Some(ref documentation) = self.documentation {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "documentation"),
                documentation.to_marked_yaml(),
            );
        }

        if let Some(ref repository) = self.repository {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "repository"),
                repository.to_marked_yaml(),
            );
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl ToMarkedYaml for Extra {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        if !self.recipe_maintainers.is_empty() {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "recipe-maintainers"),
                self.recipe_maintainers.to_marked_yaml(),
            );
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl ToMarkedYaml for IntermediateRecipe {
    fn to_marked_yaml(&self) -> MarkedNode {
        let mut mapping = MappingHash::new();

        // Add context if present
        if !self.context.is_empty() {
            let mut context_mapping = MappingHash::new();
            for (key, value) in self.context.iter() {
                context_mapping.insert(
                    MarkedScalarNode::new(Span::new_blank(), key),
                    value.to_marked_yaml(),
                );
            }
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "context"),
                MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), context_mapping)),
            );
        }

        // Add package
        mapping.insert(
            MarkedScalarNode::new(Span::new_blank(), "package"),
            self.package.to_marked_yaml(),
        );

        if !self.source.is_empty() {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "build"),
                self.source.to_marked_yaml(),
            );
        }

        mapping.insert(
            MarkedScalarNode::new(Span::new_blank(), "build"),
            self.build.to_marked_yaml(),
        );

        mapping.insert(
            MarkedScalarNode::new(Span::new_blank(), "requirements"),
            self.requirements.to_marked_yaml(),
        );

        if !self.tests.is_empty() {
            let test_nodes: Vec<MarkedNode> = self
                .tests
                .iter()
                .map(|test| test.to_marked_yaml())
                .collect();
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "tests"),
                MarkedNode::Sequence(MarkedSequenceNode::new(Span::new_blank(), test_nodes)),
            );
        }

        if let Some(ref about) = self.about {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "about"),
                about.to_marked_yaml(),
            );
        }

        if let Some(ref extra) = self.extra {
            mapping.insert(
                MarkedScalarNode::new(Span::new_blank(), "extra"),
                extra.to_marked_yaml(),
            );
        }

        MarkedNode::Mapping(MarkedMappingNode::new(Span::new_blank(), mapping))
    }
}

impl<T> ToMarkedYaml for ListOrItem<T>
where
    T: ToString,
{
    fn to_marked_yaml(&self) -> MarkedNode {
        if self.0.len() == 1 {
            MarkedNode::Scalar(MarkedScalarNode::new(
                Span::new_blank(),
                self.0[0].to_string(),
            ))
        } else {
            let nodes: Vec<MarkedNode> = self
                .0
                .iter()
                .map(|item| item.to_string())
                .map(|value| MarkedScalarNode::new(Span::new_blank(), value))
                .map(MarkedNode::Scalar)
                .collect();
            MarkedNode::Sequence(MarkedSequenceNode::new(Span::new_blank(), nodes))
        }
    }
}
