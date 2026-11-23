use minijinja::Environment;
use serde::Serialize;

#[derive(Serialize)]
pub struct BuildScriptContext {
    /// The location of the source
    pub source_dir: String,

    /// Any additional args to pass to `cargo`
    pub extra_args: Vec<String>,

    /// True if `sccache` is available.
    pub has_sccache: bool,

    /// The platform that is running the build.
    pub is_bash: bool,

    /// Verbosity flag to pass to `cargo`, if any.
    pub verbosity_flag: Option<&'static str>,
}

impl BuildScriptContext {
    pub fn render(&self) -> String {
        let env = Environment::new();
        let template = env
            .template_from_str(include_str!("build_script.j2"))
            .unwrap();
        template
            .render(minijinja::value::Serde(self))
            .unwrap()
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod test {
    fn render(
        is_bash: bool,
        has_sccache: bool,
        verbosity_flag: Option<&'static str>,
    ) -> String {
        super::BuildScriptContext {
            source_dir: String::from("my-prefix-dir"),
            extra_args: vec![],
            has_sccache,
            is_bash,
            verbosity_flag,
        }
        .render()
    }

    #[test]
    fn test_cargo_verbosity_flag() {
        let cargo_lines = [true, false]
            .into_iter()
            .flat_map(|is_bash| {
                [None, Some("-v"), Some("-vv")].map(|flag| {
                    render(is_bash, false, flag)
                        .lines()
                        .filter(|line| line.contains(" install "))
                        .collect::<String>()
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(cargo_lines, @r#"
        "$BUILD_PREFIX/bin/cargo" install --locked --root "$PREFIX" --path "my-prefix-dir" --target-dir target --no-track  --force
        "$BUILD_PREFIX/bin/cargo" install -v --locked --root "$PREFIX" --path "my-prefix-dir" --target-dir target --no-track  --force
        "$BUILD_PREFIX/bin/cargo" install -vv --locked --root "$PREFIX" --path "my-prefix-dir" --target-dir target --no-track  --force
        "%BUILD_PREFIX%\\Library\\bin\\cargo" install --locked --root "%PREFIX%" --path "my-prefix-dir" --target-dir target --no-track  --force
        "%BUILD_PREFIX%\\Library\\bin\\cargo" install -v --locked --root "%PREFIX%" --path "my-prefix-dir" --target-dir target --no-track  --force
        "%BUILD_PREFIX%\\Library\\bin\\cargo" install -vv --locked --root "%PREFIX%" --path "my-prefix-dir" --target-dir target --no-track  --force
        "#);
    }

    #[test]
    fn test_build_script_bash() {
        insta::assert_snapshot!(render(true, false, None), @r###"
        if [ -d "$PREFIX/include/openssl" ]; then
            export OPENSSL_DIR="$PREFIX"
        fi



        export CARGO="$BUILD_PREFIX/bin/cargo"
        export RUSTC="$BUILD_PREFIX/bin/rustc"
        export RUSTDOC="$BUILD_PREFIX/bin/rustdoc"

        "$BUILD_PREFIX/bin/cargo" install --locked --root "$PREFIX" --path "my-prefix-dir" --target-dir target --no-track  --force
        "###);
    }

    #[test]
    fn test_build_script_cmdexe() {
        insta::assert_snapshot!(render(false, false, None), @r###"
        if exist "%PREFIX%\Library\include\openssl" SET "OPENSSL_DIR=%PREFIX%"



        SET "CARGO=%BUILD_PREFIX%\Library\bin\cargo"
        SET "RUSTC=%BUILD_PREFIX%\Library\bin\rustc"
        SET "RUSTDOC=%BUILD_PREFIX%\Library\bin\rustdoc"

        "%BUILD_PREFIX%\\Library\\bin\\cargo" install --locked --root "%PREFIX%" --path "my-prefix-dir" --target-dir target --no-track  --force
        if errorlevel 1 exit 1
        "###);
    }

    #[test]
    fn test_sccache_bash() {
        insta::assert_snapshot!(render(true, true, None), @r###"
        if [ -d "$PREFIX/include/openssl" ]; then
            export OPENSSL_DIR="$PREFIX"
        fi

        export RUSTC_WRAPPER="sccache"

        export CARGO="$BUILD_PREFIX/bin/cargo"
        export RUSTC="$BUILD_PREFIX/bin/rustc"
        export RUSTDOC="$BUILD_PREFIX/bin/rustdoc"

        "$BUILD_PREFIX/bin/cargo" install --locked --root "$PREFIX" --path "my-prefix-dir" --target-dir target --no-track  --force

        sccache --show-stats
        "###);
    }

    #[test]
    fn test_sccache_cmdexe() {
        insta::assert_snapshot!(render(false, true, None), @r###"
        if exist "%PREFIX%\Library\include\openssl" SET "OPENSSL_DIR=%PREFIX%"

        SET "RUSTC_WRAPPER=sccache"

        SET "CARGO=%BUILD_PREFIX%\Library\bin\cargo"
        SET "RUSTC=%BUILD_PREFIX%\Library\bin\rustc"
        SET "RUSTDOC=%BUILD_PREFIX%\Library\bin\rustdoc"

        "%BUILD_PREFIX%\\Library\\bin\\cargo" install --locked --root "%PREFIX%" --path "my-prefix-dir" --target-dir target --no-track  --force
        if errorlevel 1 exit 1

        sccache --show-stats
        "###);
    }
}
