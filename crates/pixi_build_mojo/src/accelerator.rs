use std::collections::BTreeMap;

use pixi_build_backend::package_dependency::PackageDependency;
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage0::{
    Conditional, ConditionalList, IncludeExclude, Item, JinjaExpression, JinjaTemplate,
    NestedItemList, SerializableMatchSpec, SingleOutputRecipe, Value,
};
use rattler_build_types::NormalizedKey;
use rattler_conda_types::Subdir;
use rattler_virtual_packages::amdgpu::AmdGpuArchInfo;

use crate::config::{MojoBinConfig, MojoPkgConfig};

pub const VARIANT_KEY: &str = "mojo_accelerator";
const SUPPORTED_VALUES: [&str; 2] = ["cpu", "gfx1103"];

pub fn validate_variants(variants: &BTreeMap<NormalizedKey, Vec<Variable>>) -> miette::Result<()> {
    let Some(values) = variants.get(&NormalizedKey::from(VARIANT_KEY)) else {
        return Ok(());
    };
    for value in values {
        if !value
            .as_str()
            .is_some_and(|value| SUPPORTED_VALUES.contains(&value))
        {
            miette::bail!(
                "invalid value '{value}' for '{VARIANT_KEY}', supported values are: {}",
                SUPPORTED_VALUES.join(", ")
            );
        }
    }
    Ok(())
}

/// Whether the variant applies to a package. Build variants are shared by the
/// whole workspace, so other packages and platforms build as if it was unset.
pub fn applies(
    platform: Subdir,
    bins: Option<&[MojoBinConfig]>,
    pkg: Option<&MojoPkgConfig>,
) -> bool {
    platform == Subdir::Linux64 && pkg.is_none() && bins.is_some_and(|bins| !bins.is_empty())
}

pub fn configure_recipe(recipe: &mut SingleOutputRecipe) -> miette::Result<()> {
    recipe.build.files = Some(IncludeExclude::List(ConditionalList::new(vec![
        Item::Value(Value::new_concrete("bin/*".to_string(), None)),
    ])));

    recipe
        .build
        .variant
        .use_keys
        .push(Item::Value(Value::new_concrete(
            VARIANT_KEY.to_string(),
            None,
        )));
    recipe.build.variant.down_prioritize_variant =
        Some(template("${{ 1 if mojo_accelerator == 'cpu' else 0 }}")?);

    let requirements = &mut recipe.requirements;
    requirements.build.push(dependency("patchelf"));
    requirements.build.push(dependency("mojo-compiler >=1.2"));
    requirements.host.push(Item::Value(template(
        "${{ pin_compatible('mojo-compiler', exact=True) }}",
    )?));
    requirements.host.push(dependency("max-core >=26.7"));
    for provider in ["mojo-compiler", "max-core"] {
        requirements.run.push(Item::Value(template(format!(
            "${{{{ pin_compatible('{provider}', exact=True) }}}}"
        ))?));
    }
    requirements.run.push(dependency("__glibc >=2.34"));
    requirements
        .run
        .push(dependency("_x86_64-microarch-level >=3"));

    let arch = AmdGpuArchInfo::from_target_id("gfx1103")
        .expect("gfx1103 is a supported AMD architecture")
        .to_version();
    for requirement in [
        "__amdgpu".to_string(),
        format!("__amdgpu_arch =={arch}"),
        "hip-runtime-amd >=6.3.3".to_string(),
    ] {
        requirements.run.push(Item::Conditional(Conditional {
            condition: JinjaExpression::new("mojo_accelerator == 'gfx1103'".to_string())
                .map_err(miette::Error::msg)?,
            then: NestedItemList::single(dependency(&requirement)),
            else_value: None,
            condition_span: None,
        }));
    }
    Ok(())
}

pub fn template<T>(source: impl Into<String>) -> miette::Result<Value<T>> {
    Ok(Value::new_template(
        JinjaTemplate::new(source.into()).map_err(miette::Error::msg)?,
        None,
    ))
}

fn dependency(spec: &str) -> Item<SerializableMatchSpec> {
    let dependency: PackageDependency = spec
        .parse()
        .expect("backend-defined dependency specs are valid");
    Item::Value(Value::new_concrete(dependency.into(), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_build_jinja::Variable;
    use rattler_build_recipe::{
        stage0::parse_recipe_from_source,
        stage1::{Evaluate, EvaluationContext},
    };

    fn bin() -> MojoBinConfig {
        MojoBinConfig {
            name: Some("example".to_string()),
            path: Some("main.mojo".to_string()),
            extra_args: None,
        }
    }

    fn recipe() -> SingleOutputRecipe {
        let mut recipe =
            parse_recipe_from_source("package:\n  name: example\n  version: 1.0\n").unwrap();
        configure_recipe(&mut recipe).unwrap();
        recipe
    }

    fn context(variant: &str) -> EvaluationContext {
        let mut context = EvaluationContext::for_platform(Subdir::Linux64);
        context.insert(VARIANT_KEY.to_string(), Variable::from(variant));
        context
    }

    #[test]
    fn variants_are_used_for_identity_and_priority() {
        let recipe = recipe();
        for (variant, priority) in [("cpu", 1), ("gfx1103", 0)] {
            let variant_usage = recipe.build.variant.evaluate(&context(variant)).unwrap();
            assert!(variant_usage.use_keys.contains(&VARIANT_KEY.to_string()));
            assert_eq!(variant_usage.down_prioritize_variant, Some(priority));
        }
    }

    #[test]
    fn validates_variant_values() {
        let variants =
            |values: Vec<Variable>| BTreeMap::from([(NormalizedKey::from(VARIANT_KEY), values)]);
        validate_variants(&BTreeMap::new()).unwrap();
        validate_variants(&variants(vec!["cpu".into(), "gfx1103".into()])).unwrap();
        for invalid in ["CPU", "gfx1100", "amdgpu:gfx1103", ""] {
            assert!(validate_variants(&variants(vec!["cpu".into(), invalid.into()])).is_err());
        }
        let error = validate_variants(&variants(vec!["gfx1100".into()])).unwrap_err();
        insta::assert_snapshot!(error, @"invalid value 'gfx1100' for 'mojo_accelerator', supported values are: cpu, gfx1103");
    }

    #[test]
    fn amd_runtime_requirements_only_for_gfx1103() {
        let recipe = recipe();
        let run = |variant: &str| -> Vec<String> {
            recipe
                .requirements
                .evaluate(&context(variant))
                .unwrap()
                .run
                .iter()
                .map(ToString::to_string)
                .collect()
        };
        let cpu = run("cpu");
        insta::assert_debug_snapshot!(cpu, @r#"
        [
            "pin_compatible(mojo-compiler)",
            "pin_compatible(max-core)",
            "__glibc >=2.34",
            "_x86_64-microarch-level >=3",
        ]
        "#);
        let amd = run("gfx1103");
        assert_eq!(amd[..cpu.len()], cpu[..]);
        insta::assert_debug_snapshot!(amd[cpu.len()..], @r#"
        [
            "__amdgpu",
            "__amdgpu_arch ==11.0.3",
            "hip-runtime-amd >=6.3.3",
        ]
        "#);
    }

    #[test]
    fn applies_only_to_linux_64_bins() {
        let bins = [bin()];
        let pkg = MojoPkgConfig::default();
        assert!(applies(Subdir::Linux64, Some(&bins), None));
        assert!(!applies(Subdir::OsxArm64, Some(&bins), None));
        assert!(!applies(Subdir::Linux64, Some(&bins), Some(&pkg)));
        assert!(!applies(Subdir::Linux64, None, Some(&pkg)));
        assert!(!applies(Subdir::Linux64, Some(&[]), None));
    }
}
