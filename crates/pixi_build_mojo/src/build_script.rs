use super::config::{MojoBinConfig, MojoPkgConfig};
use minijinja::Environment;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct BuildScriptContext {
    /// Any executable artifacts to create.
    pub bins: Option<Vec<MojoBinConfig>>,
    /// Any packages to create.
    pub pkg: Option<MojoPkgConfig>,
    /// Whether the recipe uses the mojo_accelerator build variant.
    pub accelerator_variants_enabled: bool,
}

impl BuildScriptContext {
    pub fn render(&self) -> String {
        let env = Environment::new();
        let template = env
            .template_from_str(include_str!("build_script.j2"))
            .unwrap();
        // Normalize line endings to Unix-style for consistent output across platforms
        template
            .render(minijinja::value::Serde(self))
            .unwrap()
            .trim()
            .replace("\r\n", "\n")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bins() -> Option<Vec<MojoBinConfig>> {
        Some(vec![MojoBinConfig {
            name: Some(String::from("example")),
            path: Some(String::from("/src/main.mojo")),
            extra_args: Some(vec![String::from("-I"), String::from(".")]),
        }])
    }

    #[test]
    fn plain_bin_script() {
        let script = BuildScriptContext {
            bins: bins(),
            pkg: None,
            accelerator_variants_enabled: false,
        }
        .render();
        insta::assert_snapshot!(script, @r#"
        mojo --version

        mojo_build_args=()


        mojo build "${mojo_build_args[@]}" \
            -I . \
            "/src/main.mojo" \
            -o "$PREFIX/bin/example"
        "#);
    }

    #[test]
    fn accelerator_bin_script() {
        let script = BuildScriptContext {
            bins: bins(),
            pkg: None,
            accelerator_variants_enabled: true,
        }
        .render();
        insta::assert_snapshot!(script, @r#"
        mojo --version

        MODULAR_MOJO_MAX_LINKER_DRIVER="$(command -v "$CC")"
        export MODULAR_MOJO_MAX_LINKER_DRIVER
        export MODULAR_HOME="$PREFIX/share/max"
        export MODULAR_CACHE_DIR="$SRC_DIR/.mojo-cache"

        mojo_build_args=()


        mojo_build_args=(
            --target-cpu x86-64-v3
            --target-features ''
            -D "mojo_accelerator=${{ mojo_accelerator }}"
            -Xlinker -rpath-link
            -Xlinker "$PREFIX/lib"
        )

        case "${{ mojo_accelerator }}" in
            cpu)
                export ROCR_VISIBLE_DEVICES=-1
                export HIP_VISIBLE_DEVICES=-1
                export CUDA_VISIBLE_DEVICES=-1
                ;;
            *)
                mojo_build_args+=(--target-accelerator "${{ mojo_accelerator }}")
                ;;
        esac



        mojo build "${mojo_build_args[@]}" \
            -I . \
            "/src/main.mojo" \
            -o "$PREFIX/bin/example"
        "#);
    }

    #[test]
    fn pkg_selects_matching_command_and_extension() {
        let script = BuildScriptContext {
            bins: None,
            pkg: Some(MojoPkgConfig {
                name: Some(String::from("mylib")),
                path: Some(String::from("/src/mylib")),
                extra_args: None,
            }),
            accelerator_variants_enabled: false,
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
