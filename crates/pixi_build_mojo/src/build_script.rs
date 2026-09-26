use super::config::{MojoBinConfig, MojoPkgConfig};
use minijinja::Environment;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct BuildScriptContext {
    /// Any executable artifacts to create.
    pub bins: Option<Vec<MojoBinConfig>>,
    /// Any packages to create.
    pub pkg: Option<MojoPkgConfig>,
}

impl BuildScriptContext {
    pub fn render(&self) -> String {
        let env = Environment::new();
        let template = env
            .template_from_str(include_str!("build_script.j2"))
            .unwrap();
        // Normalize line endings to Unix-style for consistent output across platforms
        template
            .render(self)
            .unwrap()
            .trim()
            .replace("\r\n", "\n")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkg_selects_matching_command_and_extension() {
        let script = BuildScriptContext {
            bins: None,
            pkg: Some(MojoPkgConfig {
                name: Some(String::from("mylib")),
                path: Some(String::from("/src/mylib")),
                extra_args: None,
            }),
        }
        .render();

        insta::assert_snapshot!(script, @r#"
        mojo --version
        # Mojo v1.0 renamed `mojo package` to `mojo precompile`; probe which subcommand the installed compiler supports
        if mojo precompile --help >/dev/null 2>&1; then
            MOJO_PKG_CMD=precompile
            MOJO_PKG_EXT=mojoc
        else
            MOJO_PKG_CMD=package
            MOJO_PKG_EXT=mojopkg
        fi
        mkdir -p $PREFIX/lib/mojo
        mojo $MOJO_PKG_CMD  "/src/mylib" -o $PREFIX/lib/mojo/mylib.$MOJO_PKG_EXT
        "#);
    }
}
