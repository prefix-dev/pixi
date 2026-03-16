use minijinja::Environment;
use serde::Serialize;

#[derive(Serialize)]
pub struct BuildScriptContext {
    pub build_platform: BuildPlatform,
    pub source_dir: String,
    pub extra_args: Vec<String>,
    /// Whether the package has native code (influences compiler setup)
    pub has_native_code: bool,
}

#[derive(Copy, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(strum::Display))]
#[cfg_attr(test, strum(serialize_all = "snake_case"))]
pub enum BuildPlatform {
    Windows,
    Unix,
}

impl BuildScriptContext {
    pub fn render(&self) -> String {
        let env = Environment::new();
        let template = env
            .template_from_str(include_str!("build_script.j2"))
            .unwrap();
        template.render(self).unwrap().trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;

    #[rstest]
    fn test_build_script(
        #[values(BuildPlatform::Windows, BuildPlatform::Unix)] build_platform: BuildPlatform,
        #[values(true, false)] has_native_code: bool,
        #[values(vec![String::from("--no-multiarch")], vec![])] extra_args: Vec<String>,
    ) {
        let context = BuildScriptContext {
            build_platform,
            source_dir: String::from("/path/to/package"),
            extra_args: extra_args.clone(),
            has_native_code,
        };
        let script = context.render();

        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(format!(
            "{}-{}-{}",
            build_platform,
            if has_native_code { "native" } else { "pure" },
            if extra_args.is_empty() {
                "no-extra-args"
            } else {
                "with-extra-args"
            }
        ));
        settings.bind(|| {
            insta::assert_snapshot!(script);
        });
    }
}
