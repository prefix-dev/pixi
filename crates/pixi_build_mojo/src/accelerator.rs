use miette::IntoDiagnostic;
use pixi_build_backend::package_dependency::PackageDependency;
use rattler_build_recipe::stage0::{
    Conditional, ConditionalList, IncludeExclude, Item, JinjaExpression, JinjaTemplate,
    NestedItemList, SerializableMatchSpec, SingleOutputRecipe, Value,
};
use rattler_conda_types::Subdir;
use rattler_virtual_packages::amdgpu::AmdGpuArchInfo;

use crate::config::{MojoBinConfig, MojoPkgConfig};

pub const VARIANT_KEY: &str = "mojo_accelerator";

pub fn configure_recipe(
    recipe: &mut SingleOutputRecipe,
    platform: Subdir,
    bins: Option<&[MojoBinConfig]>,
    pkg: Option<&MojoPkgConfig>,
) -> miette::Result<()> {
    if platform != Subdir::Linux64 {
        miette::bail!("mojo_accelerator variants support only linux-64 executable builds");
    }
    if pkg.is_some() || bins.is_none_or(|bins| bins.is_empty()) {
        miette::bail!("mojo_accelerator variants support only executable bins, not pkg builds");
    }
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
    // Only 'cpu' and 'gfx1103' are accepted; any other value fails this lookup.
    recipe.build.variant.down_prioritize_variant = Some(template(
        "${{ {'cpu': 1, 'gfx1103': 0}[mojo_accelerator] }}",
    )?);

    let requirements = &mut recipe.requirements;
    requirements.build.push(dependency("patchelf")?);
    requirements.build.push(dependency("mojo-compiler >=1.2")?);
    requirements.host.push(Item::Value(template(
        "${{ pin_compatible('mojo-compiler', exact=True) }}",
    )?));
    requirements.host.push(dependency("max-core >=26.7")?);
    for provider in ["mojo-compiler", "max-core"] {
        requirements.run.push(Item::Value(template(format!(
            "${{{{ pin_compatible('{provider}', exact=True) }}}}"
        ))?));
    }
    requirements.run.push(dependency("__glibc >=2.34")?);
    requirements
        .run
        .push(dependency("_x86_64-microarch-level >=3")?);

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
            then: NestedItemList::single(dependency(&requirement)?),
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

fn dependency(spec: &str) -> miette::Result<Item<SerializableMatchSpec>> {
    let dependency: PackageDependency = spec.parse().into_diagnostic()?;
    Ok(Item::Value(Value::new_concrete(dependency.into(), None)))
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
        configure_recipe(&mut recipe, Subdir::Linux64, Some(&[bin()]), None).unwrap();
        recipe
    }

    fn context(variant: &str) -> EvaluationContext {
        let mut context = EvaluationContext::for_platform(Subdir::Linux64);
        context.insert(VARIANT_KEY.to_string(), Variable::from(variant));
        context
    }

    #[test]
    fn variants_are_strict_and_used_for_identity_and_priority() {
        let recipe = recipe();
        for (variant, priority) in [("cpu", 1), ("gfx1103", 0)] {
            let variant_usage = recipe.build.variant.evaluate(&context(variant)).unwrap();
            assert!(variant_usage.use_keys.contains(&VARIANT_KEY.to_string()));
            assert_eq!(variant_usage.down_prioritize_variant, Some(priority));
        }
        for invalid in ["CPU", "GFX1103", "gfx1100", "amdgpu:gfx1103", "none", ""] {
            assert!(recipe.build.variant.evaluate(&context(invalid)).is_err());
        }
        assert!(
            recipe
                .build
                .variant
                .evaluate(&EvaluationContext::new())
                .is_err()
        );
    }

    #[test]
    fn rejects_packages_and_non_linux_executables() {
        let mut recipe = recipe();
        assert!(configure_recipe(&mut recipe, Subdir::OsxArm64, Some(&[bin()]), None).is_err());
        assert!(
            configure_recipe(
                &mut recipe,
                Subdir::Linux64,
                None,
                Some(&MojoPkgConfig::default())
            )
            .is_err()
        );
    }
}
