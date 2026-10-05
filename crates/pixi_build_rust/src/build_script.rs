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
    fn render(is_bash: bool, has_sccache: bool) -> String {
        super::BuildScriptContext {
            source_dir: String::from("my-prefix-dir"),
            extra_args: vec![],
            has_sccache,
            is_bash,
        }
        .render()
    }

    #[test]
    fn test_build_script_bash() {
        insta::assert_snapshot!(render(true, false), @r###"
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
        insta::assert_snapshot!(render(false, false), @r###"
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
        insta::assert_snapshot!(render(true, true), @r###"
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
        insta::assert_snapshot!(render(false, true), @r###"
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
