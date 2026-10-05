use rattler_build_recipe::stage0::{
    ConditionalList, Item, Requirements, SerializableMatchSpec, SingleOutputRecipe,
};
use rattler_conda_types::{MatchSpec, ParseMatchSpecOptions, ParseStrictness, RepodataRevision};

pub fn generated_recipe_uses_v3(recipe: &SingleOutputRecipe) -> bool {
    !recipe.build.flags.is_empty() || requirements_use_v3(&recipe.requirements)
}

fn requirements_use_v3(requirements: &Requirements) -> bool {
    !requirements.extras.is_empty()
        || conditional_match_specs_use_v3(&requirements.build)
        || conditional_match_specs_use_v3(&requirements.host)
        || conditional_match_specs_use_v3(&requirements.run)
        || conditional_match_specs_use_v3(&requirements.run_constraints)
}

fn conditional_match_specs_use_v3(list: &ConditionalList<SerializableMatchSpec>) -> bool {
    list.iter().any(item_uses_v3_match_spec)
}

fn item_uses_v3_match_spec(item: &Item<SerializableMatchSpec>) -> bool {
    match item {
        Item::Value(value) => value
            .as_concrete()
            .is_some_and(|spec| match_spec_uses_v3(&spec.0)),
        Item::Conditional(conditional) => {
            conditional.then.iter().any(item_uses_v3_match_spec)
                || conditional
                    .else_value
                    .as_ref()
                    .is_some_and(|else_value| else_value.iter().any(item_uses_v3_match_spec))
        }
    }
}

fn match_spec_uses_v3(spec: &MatchSpec) -> bool {
    spec.required_repodata_revision() == RepodataRevision::V3
}

pub fn recipe_source_uses_v3(source: &str) -> bool {
    serde_yaml::from_str::<serde_yaml::Value>(source)
        .ok()
        .is_some_and(|value| yaml_value_uses_v3(&value))
}

fn yaml_value_uses_v3(value: &serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Mapping(mapping) => {
            let has_v3_build_flags = mapping
                .get(serde_yaml::Value::String("build".to_string()))
                .is_some_and(yaml_build_uses_v3);
            let has_v3_requirements = mapping
                .get(serde_yaml::Value::String("requirements".to_string()))
                .is_some_and(yaml_requirements_use_v3);
            let has_v3_output = mapping
                .get(serde_yaml::Value::String("outputs".to_string()))
                .is_some_and(yaml_value_uses_v3);

            has_v3_build_flags
                || has_v3_requirements
                || has_v3_output
                || mapping.values().any(yaml_value_uses_v3)
        }
        serde_yaml::Value::Sequence(values) => values.iter().any(yaml_value_uses_v3),
        _ => false,
    }
}

fn yaml_build_uses_v3(value: &serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Mapping(mapping) => mapping
            .get(serde_yaml::Value::String("flags".to_string()))
            .is_some_and(|flags| !yaml_is_empty(flags)),
        _ => false,
    }
}

fn yaml_requirements_use_v3(value: &serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Mapping(mapping) => {
            mapping
                .get(serde_yaml::Value::String("extras".to_string()))
                .is_some_and(|extras| !yaml_is_empty(extras))
                || ["build", "host", "run", "run_constraints"]
                    .iter()
                    .any(|key| {
                        mapping
                            .get(serde_yaml::Value::String((*key).to_string()))
                            .is_some_and(yaml_requirement_list_uses_v3)
                    })
        }
        _ => false,
    }
}

fn yaml_requirement_list_uses_v3(value: &serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Sequence(values) => values.iter().any(yaml_requirement_list_uses_v3),
        serde_yaml::Value::Mapping(mapping) => {
            mapping.keys().any(|key| {
                matches!(
                    key.as_str(),
                    Some("extras" | "flags" | "when" | "condition")
                )
            }) || mapping.values().any(yaml_requirement_list_uses_v3)
        }
        serde_yaml::Value::String(spec) => requirement_string_uses_v3(spec),
        _ => false,
    }
}

fn requirement_string_uses_v3(spec: &str) -> bool {
    let options = ParseMatchSpecOptions::new(ParseStrictness::Strict)
        .with_repodata_revision(RepodataRevision::V3);
    MatchSpec::from_str(spec, options)
        .ok()
        .is_some_and(|spec| match_spec_uses_v3(&spec))
        || spec.contains("extras=")
        || spec.contains("flags=")
        || spec.contains("when=")
}

fn yaml_is_empty(value: &serde_yaml::Value) -> bool {
    match value {
        serde_yaml::Value::Null => true,
        serde_yaml::Value::Sequence(values) => values.is_empty(),
        serde_yaml::Value::Mapping(mapping) => mapping.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_recipe_source_does_not_use_v3() {
        let source = r#"
package:
  name: example
  version: 0.1.0
requirements:
  run:
    - python >=3.12
"#;

        assert!(!recipe_source_uses_v3(source));
    }

    #[test]
    fn recipe_source_uses_v3_for_requirements_extras() {
        let source = r#"
package:
  name: example
  version: 0.1.0
requirements:
  extras:
    test:
      - pytest
"#;

        assert!(recipe_source_uses_v3(source));
    }

    #[test]
    fn recipe_source_uses_v3_for_matchspec_fields() {
        let source = r#"
package:
  name: example
  version: 0.1.0
requirements:
  run:
    - python[extras=[dev], flags=[cuda]]
"#;

        assert!(recipe_source_uses_v3(source));
    }

    #[test]
    fn recipe_source_uses_v3_for_build_flags() {
        let source = r#"
package:
  name: example
  version: 0.1.0
build:
  flags:
    - cuda
"#;

        assert!(recipe_source_uses_v3(source));
    }
}
