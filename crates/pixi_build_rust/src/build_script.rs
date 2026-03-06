use minijinja::Environment;
use serde::Serialize;

#[derive(Serialize)]
pub struct BuildScriptContext {
    /// The location of the source
    pub source_dir: String,

    /// Any additional args to pass to `cargo`
    pub extra_args: Vec<String>,

    /// True if `openssl` is part of the build environment
    pub has_openssl: bool,

    /// True if `sccache` is available.
    pub has_sccache: bool,

    /// The platform that is running the build.
    pub is_bash: bool,
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

    #[rstest]
    fn test_build_script(#[values(true, false)] is_bash: bool) {
        let context = super::BuildScriptContext {
            source_dir: String::from("my-prefix-dir"),
            extra_args: vec![],
            has_openssl: false,
            has_sccache: false,
            is_bash,
        };
        let script = context.render();

        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(if is_bash { "bash" } else { "cmdexe" });
        settings.bind(|| {
            insta::assert_snapshot!(script);
        });
    }

    #[rstest]
    fn test_sccache(#[values(true, false)] is_bash: bool) {
        let context = super::BuildScriptContext {
            source_dir: String::from("my-prefix-dir"),
            extra_args: vec![],
            has_openssl: false,
            has_sccache: true,
            is_bash,
        };
        let script = context.render();

        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(if is_bash { "bash" } else { "cmdexe" });
        settings.bind(|| {
            insta::assert_snapshot!(script);
        });
    }

    #[rstest]
    fn test_openssl(#[values(true, false)] is_bash: bool) {
        let context = super::BuildScriptContext {
            source_dir: String::from("my-prefix-dir"),
            extra_args: vec![],
            has_openssl: true,
            has_sccache: false,
            is_bash,
        };
        let script = context.render();

        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(if is_bash { "bash" } else { "cmdexe" });
        settings.bind(|| {
            insta::assert_snapshot!(script);
        });
    }
}
