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
const CPU: &str = "cpu";
/// AMD targets listed at <https://max.modular.com/gpu/hardware-compatibility/>,
/// with the minimum HIP runtime each needs.
const AMD_TARGETS: [(&str, &str); 14] = [
    ("gfx90a", "6.3.3"),
    ("gfx942", "6.3.3"),
    ("gfx950", "7.0"),
    ("gfx1030", "6.3.3"),
    ("gfx1033", "6.3.3"),
    ("gfx1100", "6.3.3"),
    ("gfx1101", "6.3.3"),
    ("gfx1102", "6.3.3"),
    ("gfx1103", "6.3.3"),
    ("gfx1150", "6.3.3"),
    ("gfx1151", "6.3.3"),
    ("gfx1152", "6.3.3"),
    ("gfx1200", "6.3.3"),
    ("gfx1201", "6.3.3"),
];

fn supported_values() -> impl Iterator<Item = &'static str> {
    std::iter::once(CPU).chain(AMD_TARGETS.iter().map(|(target, _)| *target))
}

pub fn validate_variants(variants: &BTreeMap<NormalizedKey, Vec<Variable>>) -> miette::Result<()> {
    let Some(values) = variants.get(&NormalizedKey::from(VARIANT_KEY)) else {
        return Ok(());
    };
    for value in values {
        if !value
            .as_str()
            .is_some_and(|value| supported_values().any(|supported| supported == value))
        {
            miette::bail!(
                "invalid value '{value}' for '{VARIANT_KEY}', supported values are: {}",
                supported_values().collect::<Vec<_>>().join(", ")
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
    // TODO: Adapt these runtime providers and pins when the Mojo and MAX packages are split up.
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

    for (target, hip_runtime) in AMD_TARGETS {
        let arch = AmdGpuArchInfo::from_target_id(target)
            .expect("backend-defined AMD targets are valid")
            .to_version();
        requirements.run.push(Item::Conditional(Conditional {
            condition: JinjaExpression::new(format!("mojo_accelerator == '{target}'"))
                .map_err(miette::Error::msg)?,
            then: NestedItemList::new(vec![
                dependency("__amdgpu"),
                dependency(&format!("__amdgpu_arch =={arch}")),
                dependency(&format!("hip-runtime-amd >={hip_runtime}")),
            ]),
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
        for invalid in ["CPU", "gfx1103:xnack-", "gfx1036", "amdgpu:gfx1103", ""] {
            assert!(validate_variants(&variants(vec!["cpu".into(), invalid.into()])).is_err());
        }
        let error = validate_variants(&variants(vec!["gfx1036".into()])).unwrap_err();
        insta::assert_snapshot!(error, @"invalid value 'gfx1036' for 'mojo_accelerator', supported values are: cpu, gfx90a, gfx942, gfx950, gfx1030, gfx1033, gfx1100, gfx1101, gfx1102, gfx1103, gfx1150, gfx1151, gfx1152, gfx1200, gfx1201");
    }

    #[test]
    fn amd_runtime_requirements_per_target() {
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
        let amd = AMD_TARGETS
            .iter()
            .map(|(target, _)| {
                let run = run(target);
                assert_eq!(run[..cpu.len()], cpu[..]);
                format!("{target}: {}", run[cpu.len()..].join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(amd, @"
        gfx90a: __amdgpu, __amdgpu_arch ==9.0.10, hip-runtime-amd >=6.3.3
        gfx942: __amdgpu, __amdgpu_arch ==9.4.2, hip-runtime-amd >=6.3.3
        gfx950: __amdgpu, __amdgpu_arch ==9.5.0, hip-runtime-amd >=7.0
        gfx1030: __amdgpu, __amdgpu_arch ==10.3.0, hip-runtime-amd >=6.3.3
        gfx1033: __amdgpu, __amdgpu_arch ==10.3.3, hip-runtime-amd >=6.3.3
        gfx1100: __amdgpu, __amdgpu_arch ==11.0.0, hip-runtime-amd >=6.3.3
        gfx1101: __amdgpu, __amdgpu_arch ==11.0.1, hip-runtime-amd >=6.3.3
        gfx1102: __amdgpu, __amdgpu_arch ==11.0.2, hip-runtime-amd >=6.3.3
        gfx1103: __amdgpu, __amdgpu_arch ==11.0.3, hip-runtime-amd >=6.3.3
        gfx1150: __amdgpu, __amdgpu_arch ==11.5.0, hip-runtime-amd >=6.3.3
        gfx1151: __amdgpu, __amdgpu_arch ==11.5.1, hip-runtime-amd >=6.3.3
        gfx1152: __amdgpu, __amdgpu_arch ==11.5.2, hip-runtime-amd >=6.3.3
        gfx1200: __amdgpu, __amdgpu_arch ==12.0.0, hip-runtime-amd >=6.3.3
        gfx1201: __amdgpu, __amdgpu_arch ==12.0.1, hip-runtime-amd >=6.3.3
        ");
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
