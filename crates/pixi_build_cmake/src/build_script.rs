use minijinja::Environment;
use serde::Serialize;

#[derive(Serialize)]
pub struct BuildScriptContext {
    pub build_platform: BuildPlatform,
    pub source_dir: String,
    pub extra_args: Vec<String>,
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
        #[values(vec![String::from("test-arg")], vec![])] extra_args: Vec<String>,
    ) {
        let context = BuildScriptContext {
            build_platform,
            source_dir: String::from("my-prefix-dir"),
            extra_args: extra_args.clone(),
        };
        let script = context.render();

        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(format!(
            "{}-{}",
            build_platform,
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

    /// Renders the script for a platform.
    fn render(build_platform: BuildPlatform) -> String {
        BuildScriptContext {
            build_platform,
            source_dir: String::from("/src/demo"),
            extra_args: vec![],
        }
        .render()
    }

    /// An install prefix holding a space must reach cmake as one argument, so
    /// the value has to be quoted in the script.
    #[test]
    fn test_the_install_prefix_is_quoted() {
        assert!(
            render(BuildPlatform::Unix).contains(r#"-DCMAKE_INSTALL_PREFIX="$PREFIX""#),
            "the unix install prefix is unquoted"
        );
        assert!(
            render(BuildPlatform::Windows).contains(r#"-DCMAKE_INSTALL_PREFIX="%LIBRARY_PREFIX%""#),
            "the windows install prefix is unquoted"
        );
    }

    /// Runs a rendered script with stubs on PATH instead of a real toolchain,
    /// and returns the arguments the configure step passed to cmake.
    ///
    /// This is what proves the quoting works rather than merely being present:
    /// the shell, not a string comparison, decides where the arguments split.
    #[cfg(unix)]
    fn configure_arguments(prefix: &str) -> Vec<String> {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("failed to create a temp dir");
        let record = dir.path().join("argv");

        let write_stub = |name: &str, body: &str| {
            let path = dir.path().join(name);
            fs_err::write(&path, body).expect("failed to write a stub");
            let mut permissions = fs_err::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs_err::set_permissions(&path, permissions).expect("failed to chmod a stub");
        };
        write_stub(
            "cmake",
            "#!/bin/sh\n{ echo '--- invocation'; for arg in \"$@\"; do echo \"$arg\"; done; } >> \"$ARGV_RECORD\"\nexit 0\n",
        );
        write_stub("ninja", "#!/bin/sh\nexit 0\n");

        let script = dir.path().join("build.sh");
        fs_err::write(&script, render(BuildPlatform::Unix)).expect("failed to write the script");

        let output = std::process::Command::new("bash")
            .arg(&script)
            .current_dir(dir.path())
            .env("ARGV_RECORD", &record)
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
            .env("PREFIX", prefix)
            .env("CMAKE_ARGS", "")
            .env("PYTHON", dir.path().join("no-such-python"))
            .output()
            .expect("failed to run the script");
        assert!(
            output.status.success(),
            "the script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // The configure call is the invocation that names the source directory.
        let recorded = fs_err::read_to_string(&record).expect("the stub recorded nothing");
        recorded
            .split("--- invocation\n")
            .map(|invocation| {
                invocation
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<String>>()
            })
            .find(|arguments| arguments.iter().any(|argument| argument == "-S"))
            .expect("no configure invocation was recorded")
    }

    #[cfg(unix)]
    #[test]
    fn test_configure_gets_a_spaced_prefix_as_one_argument() {
        let arguments = configure_arguments("/tmp/a prefix with spaces");

        assert!(
            arguments.contains(&"-DCMAKE_INSTALL_PREFIX=/tmp/a prefix with spaces".to_string()),
            "the prefix was split up: {arguments:?}"
        );
    }
}
