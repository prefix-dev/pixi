use minijinja::Environment;
use serde::Serialize;

#[derive(Serialize)]
pub struct BuildScriptContext {
    pub build_platform: BuildPlatform,
    pub source_dir: String,
    pub extra_args: Vec<String>,
    /// The package has a host dependency on Python.
    /// This is used to determine if the build script
    /// should include Python-related logic.
    pub has_host_python: bool,
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
mod test {
    use rstest::*;

    use super::*;

    #[rstest]
    fn test_build_script(
        #[values(BuildPlatform::Windows, BuildPlatform::Unix)] build_platform: BuildPlatform,
        #[values(true, false)] has_host_python: bool,
        #[values(vec![String::from("test-arg")], vec![])] extra_args: Vec<String>,
    ) {
        let context = BuildScriptContext {
            build_platform,
            source_dir: String::from("my-prefix-dir"),
            extra_args: extra_args.clone(),
            has_host_python,
        };
        let script = context.render();

        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(format!(
            "{}-{}-{}",
            build_platform,
            if has_host_python {
                "python"
            } else {
                "nopython"
            },
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
