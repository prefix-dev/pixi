use std::{
    collections::HashMap,
    fs,
    io::{ErrorKind, Write},
    path::Path,
    str::FromStr,
};

use miette::{Context, IntoDiagnostic};
use minijinja::{Environment, context};
use pixi_config::{Config, get_default_author, pixi_home};
use pixi_consts::consts;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::{FeatureName, pyproject::PyProjectManifest};
use pixi_utils::conda_environment_file::CondaEnvFile;
use rattler_conda_types::{NamedChannelOrUrl, Platform};
use same_file::is_same_file;
use tokio::fs::OpenOptions;
use url::Url;
use uv_normalize::PackageName;

use crate::interface::Interface;

mod options;
mod template;

pub use options::{GitAttributes, InitOptions, ManifestFormat};

pub async fn init<I: Interface>(interface: &I, options: InitOptions) -> miette::Result<Workspace> {
    let env = Environment::new();
    // Fail silently if the directory already exists or cannot be created.
    fs_err::create_dir_all(&options.path).into_diagnostic()?;
    let dir = dunce::canonicalize(options.path).into_diagnostic()?;
    let pixi_manifest_path = dir.join(consts::WORKSPACE_MANIFEST);
    let pyproject_manifest_path = dir.join(consts::PYPROJECT_MANIFEST);
    let mojoproject_manifest_path = dir.join(consts::MOJOPROJECT_MANIFEST);
    let gitignore_path = dir.join(".gitignore");
    let gitattributes_path = dir.join(".gitattributes");
    let config = Config::load(&dir);

    if is_init_dir_equal_to_pixi_home_parent(&dir) {
        let help_msg = if interface.is_cli().await {
            format!(
                "Please follow the getting started guide at https://pixi.sh/v{}/init_getting_started/ or run the following command to create a new workspace in a subdirectory:\n\n  {}\n",
                consts::PIXI_VERSION,
                console::style("pixi init my_workspace").bold(),
            )
        } else {
            "You have to select a subdirectory to create a new workspace".to_string()
        };
        miette::bail!(
            help = help_msg,
            "initialization without a name in the parent directory of your PIXI_HOME is not allowed.",
        );
    }

    let default_name = get_name_from_dir(&dir).unwrap_or_else(|_| String::from("new_workspace"));
    let version = "0.1.0";
    let author = get_default_author();
    let platforms = if options.platforms.is_empty() {
        vec![Platform::current().to_string()]
    } else {
        options.platforms.clone()
    };

    let index_url = config.pypi_config.index_url.clone();
    let extra_index_urls = config.pypi_config.extra_index_urls.clone();

    // Create a 'pixi.toml' manifest and populate it by importing a conda
    // environment file
    let workspace = if let Some(env_file_path) = options.env_file {
        // Check if the 'pixi.toml' file doesn't already exist. We don't want to
        // overwrite it.
        if pixi_manifest_path.is_file() {
            miette::bail!("{} already exists", consts::WORKSPACE_MANIFEST);
        }

        let env_file = CondaEnvFile::from_path(&env_file_path)?;
        let name = env_file
            .name()
            .unwrap_or(default_name.clone().as_str())
            .to_string();

        let env_vars = env_file.variables();

        // TODO: Improve this:
        //  - Use .condarc as channel config
        let (conda_deps, pypi_deps, channels) = env_file.to_manifest(&config)?;
        let rendered_workspace_template = render_workspace(
            &env,
            name,
            version,
            author.as_ref(),
            channels,
            &platforms,
            index_url.as_ref(),
            &extra_index_urls,
            config.s3_options,
            Some(&env_vars),
            options.conda_pypi_mapping.as_ref(),
        );
        let mut workspace =
            WorkspaceMut::from_template(pixi_manifest_path, rendered_workspace_template)?;
        workspace.add_specs(
            conda_deps,
            pypi_deps,
            &[] as &[Platform],
            &FeatureName::default(),
        )?;
        let workspace = workspace.save().await.into_diagnostic()?;

        interface
            .success(&format!(
                "Created {}",
                // Canonicalize the path to make it more readable, but if it fails just use the path as
                // is.
                workspace.workspace.provenance.path.display()
            ))
            .await;

        workspace
    } else {
        let channels = if let Some(channels) = options.channels {
            channels
        } else {
            config.default_channels().to_vec()
        };

        // Dialog with user to create a 'pyproject.toml' or 'pixi.toml' manifest
        // If nothing is defined but there is a `pyproject.toml` file, ask the user.
        let pyproject = if !pixi_manifest_path.is_file()
            && options.format.is_none()
            && pyproject_manifest_path.is_file()
        {
            interface.confirm(&format!(
                "A '{}' file already exists. Do you want to extend it with the '{}' configuration?",
                console::style(consts::PYPROJECT_MANIFEST).bold(),
                console::style("[tool.pixi]").bold().green()
            )).await?
        } else {
            options.format == Some(ManifestFormat::Pyproject)
        };

        // Inject a tool.pixi.workspace section into an existing pyproject.toml file if
        // there is one without '[tool.pixi.workspace]'
        if pyproject && pyproject_manifest_path.is_file() {
            let pyproject = PyProjectManifest::from_path(&pyproject_manifest_path)?;

            // Early exit if 'pyproject.toml' already contains a '[tool.pixi.workspace]' table
            if pyproject.has_pixi_table() {
                interface.info("Nothing to do here: 'pyproject.toml' already contains a '[tool.pixi.workspace]' section.").await;
                let workspace = Workspace::from_path(&pyproject_manifest_path)?;
                return Ok(workspace);
            }

            let (name, pixi_name) = match pyproject.name() {
                Some(name) => (name.to_string(), false),
                None => (get_pypi_safe_name(&default_name), true),
            };
            let environments = pyproject.environments_from_groups(&dir).into_diagnostic()?;
            let rv = env
                .render_named_str(
                    consts::PYPROJECT_MANIFEST,
                    template::PYROJECT_TEMPLATE_EXISTING,
                    context! {
                        name,
                        pixi_name,
                        channels,
                        platforms,
                        environments,
                        index_url => index_url.as_ref(),
                        extra_index_urls => &extra_index_urls,
                        s3 => relevant_s3_options(config.s3_options, channels),
                    },
                )
                .expect("should be able to render the template");
            if let Err(e) = {
                fs::OpenOptions::new()
                    .append(true)
                    .open(pyproject_manifest_path.clone())
                    .and_then(|mut p| p.write_all(rv.as_bytes()))
            } {
                tracing::warn!(
                    "Warning, couldn't update '{}' because of: {}",
                    pyproject_manifest_path.to_string_lossy(),
                    e
                );
            } else {
                // Inform about the addition of the package itself as an editable dependency of
                // the workspace
                interface
                    .success(&format!(
                        "Added package '{name}' as an editable dependency."
                    ))
                    .await;
                // Inform about the addition of environments from optional dependencies
                // or dependency groups (if any)
                if !environments.is_empty() {
                    let envs: Vec<&str> = environments.keys().map(AsRef::as_ref).collect();
                    interface.success(&format!(
                        "Added environment{} '{}' from optional dependencies or dependency groups.",
                        if envs.len() > 1 { "s" } else { "" },
                        envs.join("', '")
                    )).await;
                }
            }

            Workspace::from_path(&pyproject_manifest_path)?

            // Create a 'pyproject.toml' manifest
        } else if pyproject {
            // PyPI package names must not start or end with '-', '_', or '.',
            // so strip those boundary characters before normalizing.
            let pypi_safe_name = get_pypi_safe_name(&default_name);
            // Normalize separators to '-' as PyPI dist-info convention requires.
            let pypi_package_name = PackageName::from_str(&pypi_safe_name)
                .map(|name| name.as_dist_info_name().to_string())
                .unwrap_or_else(|_| pypi_safe_name.clone());

            let rv = env
                .render_named_str(
                    consts::PYPROJECT_MANIFEST,
                    template::NEW_PYROJECT_TEMPLATE,
                    context! {
                        name => pypi_safe_name,
                        pypi_package_name,
                        version,
                        author,
                        channels,
                        platforms,
                        index_url => index_url.as_ref(),
                        extra_index_urls => &extra_index_urls,
                        s3 => relevant_s3_options(config.s3_options, channels),
                    },
                )
                .expect("should be able to render the template");
            save_manifest_file(interface, &pyproject_manifest_path, rv).await?;

            let src_dir = dir.join("src").join(pypi_package_name);
            tokio::fs::create_dir_all(&src_dir)
                .await
                .into_diagnostic()
                .wrap_err_with(|| format!("Could not create directory {}.", src_dir.display()))?;

            let init_file = src_dir.join("__init__.py");
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&init_file)
                .await
            {
                Ok(_) => (),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    // If the file already exists, do nothing
                }
                Err(e) => {
                    return Err(e).into_diagnostic().wrap_err_with(|| {
                        format!("Could not create file {}.", init_file.display())
                    });
                }
            };

            Workspace::from_path(&pyproject_manifest_path)?
        // Create a 'pixi.toml' manifest
        } else {
            let (path, file_name) = if options.format == Some(ManifestFormat::Mojoproject) {
                (mojoproject_manifest_path, consts::MOJOPROJECT_MANIFEST)
            } else {
                (pixi_manifest_path, consts::WORKSPACE_MANIFEST)
            };

            // Check if the manifest file doesn't already exist. We don't want to
            // overwrite it.
            if path.is_file() {
                miette::bail!("{} already exists", file_name);
            }

            let rv = render_workspace(
                &env,
                default_name,
                version,
                author.as_ref(),
                channels,
                &platforms,
                index_url.as_ref(),
                &extra_index_urls,
                config.s3_options,
                None,
                options.conda_pypi_mapping.as_ref(),
            );
            save_manifest_file(interface, &path, rv).await?;
            Workspace::from_path(&path)?
        }
    };

    // create a .gitignore if one is missing
    if let Err(e) =
        create_or_append_file(&gitignore_path, template::GITIGNORE_TEMPLATE.trim_start())
    {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            gitignore_path.to_string_lossy(),
            e
        );
    }

    let git_attributes = options.scm.unwrap_or(GitAttributes::Github);

    // create a .gitattributes if one is missing
    if let Err(e) = create_or_append_file(&gitattributes_path, git_attributes.template()) {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            gitattributes_path.to_string_lossy(),
            e
        );
    }

    Ok(workspace)
}

fn is_init_dir_equal_to_pixi_home_parent(init_dir: &Path) -> bool {
    pixi_home()
        .as_ref()
        .and_then(|home_dir| home_dir.parent())
        .and_then(|parent| is_same_file(parent, init_dir).ok())
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn render_workspace(
    env: &Environment<'_>,
    name: String,
    version: &str,
    author: Option<&(String, String)>,
    channels: Vec<NamedChannelOrUrl>,
    platforms: &Vec<String>,
    index_url: Option<&Url>,
    extra_index_urls: &Vec<Url>,
    s3_options: HashMap<String, pixi_config::S3Options>,
    env_vars: Option<&HashMap<String, String>>,
    pypi_mapping: Option<&HashMap<NamedChannelOrUrl, String>>,
) -> String {
    let ctx = context! {
        name,
        version,
        author,
        channels,
        platforms,
        index_url,
        extra_index_urls,
        s3 => relevant_s3_options(s3_options, channels),
        env_vars => {if let Some(env_vars) = env_vars {
            env_vars.iter().map(|(k, v)| format!("{k} = \"{v}\"")).collect::<Vec<String>>().join(", ")
        } else {String::new()}},
        pypi_mapping => {if let Some(pypi_mapping) = pypi_mapping {
            if pypi_mapping.is_empty() {
                String::from(" ")
            } else {
                pypi_mapping.iter().map(|(k, v)| format!("\"{k}\" = \"{v}\"")).collect::<Vec<String>>().join(", ")
            }
        } else {String::new()}},
    };

    env.render_named_str(
        consts::WORKSPACE_MANIFEST,
        template::WORKSPACE_TEMPLATE,
        ctx,
    )
    .expect("should be able to render the template")
}

fn relevant_s3_options(
    s3_options: HashMap<String, pixi_config::S3Options>,
    channels: Vec<NamedChannelOrUrl>,
) -> HashMap<String, pixi_config::S3Options> {
    // only take s3 options in manifest if they are used in the default channels
    let s3_buckets = channels
        .iter()
        .filter_map(|channel| match channel {
            NamedChannelOrUrl::Name(_) => None,
            NamedChannelOrUrl::Path(_) => None,
            NamedChannelOrUrl::Url(url) => {
                if url.scheme() == "s3" {
                    url.host().map(|host| host.to_string())
                } else {
                    None
                }
            }
        })
        .collect::<Vec<_>>();

    s3_options
        .into_iter()
        .filter(|(key, _)| s3_buckets.contains(key))
        .collect()
}

/// Save the rendered template to a file, and print a message to the user.
async fn save_manifest_file<I: Interface>(
    interface: &I,
    path: &Path,
    content: String,
) -> miette::Result<()> {
    fs_err::write(path, content).into_diagnostic()?;
    interface
        .success(&format!(
            "Created {}",
            // Canonicalize the path to make it more readable, but if it fails just use the path as is.
            dunce::canonicalize(path)
                .unwrap_or(path.to_path_buf())
                .display()
        ))
        .await;
    Ok(())
}

fn get_name_from_dir(path: &Path) -> miette::Result<String> {
    Ok(path
        .file_name()
        .ok_or(miette::miette!(
            "Cannot get file or directory name from the path: {}",
            path.to_string_lossy()
        ))?
        .to_string_lossy()
        .to_string())
}

// When the specific template is not in the file or the file does not exist.
// Make the file and append the template to the file.
fn create_or_append_file(path: &Path, template: &str) -> std::io::Result<()> {
    let file = fs_err::read_to_string(path).unwrap_or_default();

    if !file.contains(template) {
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?
            .write_all(template.as_bytes())?;
    }
    Ok(())
}

fn get_pypi_safe_name(name: &str) -> String {
    // PyPI package names must not start or end with '-', '_', or '.'
    // so strip those boundary characters before normalizing.
    let trimmed = name.trim_matches(|c: char| matches!(c, '_' | '-' | '.'));
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Read, path::Path};

    use rstest::rstest;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_create_or_append_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file.txt");
        let template = "Test Template";

        fn read_file_content(path: &Path) -> String {
            let mut file = fs_err::File::open(path).unwrap();
            let mut content = String::new();
            file.read_to_string(&mut content).unwrap();
            content
        }

        // Scenario 1: File does not exist.
        create_or_append_file(&file_path, template).unwrap();
        assert_eq!(read_file_content(&file_path), template);

        // Scenario 2: File exists but doesn't contain the template.
        create_or_append_file(&file_path, "New Content").unwrap();
        assert!(read_file_content(&file_path).contains(template));
        assert!(read_file_content(&file_path).contains("New Content"));

        // Scenario 3: File exists and already contains the template.
        let original_content = read_file_content(&file_path);
        create_or_append_file(&file_path, template).unwrap();
        assert_eq!(read_file_content(&file_path), original_content);

        // Scenario 4: Path is a folder not a file, give an error.
        assert!(create_or_append_file(dir.path(), template).is_err());

        dir.close().unwrap();
    }

    struct MockInterface {
        pub confirm_response: bool,
    }

    impl Interface for MockInterface {
        async fn is_cli(&self) -> bool {
            false
        }
        async fn confirm(&self, _msg: &str) -> miette::Result<bool> {
            Ok(self.confirm_response)
        }
        async fn error(&self, _msg: &str) {}
        async fn info(&self, _msg: &str) {}
        async fn success(&self, _msg: &str) {}
        async fn warning(&self, _msg: &str) {}
    }

    #[derive(Default)]
    struct TestConfig {
        pub format: Option<ManifestFormat>,
        pub pre_existing_pixi: bool,
        pub pre_existing_pyproject: bool,
        pub pre_existing_mojo: bool,
        pub with_env_file: bool,
        pub confirm_response: bool,
        pub pyproject_already_extended: bool,
    }

    struct TestOutcome {
        pub result: miette::Result<Workspace>,
        pub project_path: std::path::PathBuf,
        pub pixi_exists: bool,
        pub pyproject_exists: bool,
        pub mojo_exists: bool,
        pub _tmp_dir: tempfile::TempDir,
    }

    // Create TestOutcome function
    async fn run_init_scenario(config: TestConfig) -> TestOutcome {
        let tmp_dir = tempfile::tempdir().unwrap();
        let project_path = tmp_dir.path().to_path_buf();

        if config.pre_existing_pyproject {
            let pyproject_path = project_path.join(consts::PYPROJECT_MANIFEST);
            let content = if config.pyproject_already_extended {
                "[workspace]\nname = \"existing_pixi\"\nchannels = []\nplatforms = []\n\n[tool.pixi.workspace]\nchannels = []"
            } else {
                "[workspace]\nname = \"existing_pixi\"\nchannels = []\nplatforms = []"
            };
            fs_err::write(&pyproject_path, content).unwrap();
        }

        if config.pre_existing_pixi {
            let pixi_path = project_path.join(consts::WORKSPACE_MANIFEST);
            fs_err::write(
                &pixi_path,
                "[workspace]\nname = \"existing_pixi\"\nchannels = []\nplatforms = []",
            )
            .unwrap();
        }

        if config.pre_existing_mojo {
            let mojo_path = project_path.join(consts::MOJOPROJECT_MANIFEST);
            fs_err::write(
                &mojo_path,
                "[workspace]\nname = \"existing_pixi\"\nchannels = []\nplatforms = []",
            )
            .unwrap();
        }

        let mut env_file = None;
        if config.with_env_file {
            let env_path = project_path.join("environment.yml");
            fs_err::write(
                &env_path,
                "name: env\nchannels: [conda-forge]\ndependencies: [python]",
            )
            .unwrap();
            env_file = Some(env_path);
        }

        let options = InitOptions {
            path: project_path.clone(),
            env_file,
            format: config.format,
            channels: None,
            platforms: vec![],
            scm: None,
            conda_pypi_mapping: None,
        };

        let interface = MockInterface {
            confirm_response: config.confirm_response,
        };
        let result = init(&interface, options).await;

        let pixi_exists = project_path.join(consts::WORKSPACE_MANIFEST).is_file();
        let pyproject_exists = project_path.join(consts::PYPROJECT_MANIFEST).is_file();
        let mojo_exists = project_path.join(consts::MOJOPROJECT_MANIFEST).is_file();

        TestOutcome {
            result,
            project_path,
            pixi_exists,
            pyproject_exists,
            mojo_exists,
            _tmp_dir: tmp_dir,
        }
    }

    #[rstest]
    #[tokio::test]
    async fn test_init_with_env_file_fail_if_pixi_exists(
        #[values(
            None,
            Some(ManifestFormat::Pixi),
            Some(ManifestFormat::Pyproject),
            Some(ManifestFormat::Mojoproject)
        )]
        format: Option<ManifestFormat>,
        #[values(true, false)] pre_existing_pyproject: bool,
        #[values(true, false)] pre_existing_mojo: bool,
        #[values(true, false)] confirm_response: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format,
            with_env_file: true,
            pre_existing_pixi: true,
            pre_existing_pyproject,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        let error = outcome.result.unwrap_err();
        let error_msg = format!("{:?}", error);

        assert!(
            error_msg.contains("pixi.toml already exists"),
            "The command failed, but for the wrong reason. Error texts was: {}",
            error_msg
        )
    }

    #[rstest]
    #[tokio::test]

    async fn test_init_with_env_file_succeeds(
        #[values(
            None,
            Some(ManifestFormat::Pixi),
            Some(ManifestFormat::Pyproject),
            Some(ManifestFormat::Mojoproject)
        )]
        format: Option<ManifestFormat>,
        #[values(true, false)] pre_existing_pyproject: bool,
        #[values(true, false)] pre_existing_mojo: bool,
        #[values(true, false)] confirm_response: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format,
            with_env_file: true,
            pre_existing_pixi: false,
            pre_existing_pyproject,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(outcome.pixi_exists);
    }

    #[rstest]
    #[tokio::test]

    async fn test_prompt_for_extending_pyproject_confirm_yes_not_extended(
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject: true,
            pre_existing_mojo,
            confirm_response: true,
            pyproject_already_extended: false,
        })
        .await;

        // let workspace = outcome.result.unwrap();
        assert!(outcome.result.is_ok());
        assert!(outcome.pyproject_exists);

        // check that the relevant table was added to pyproject file
        let pyproject_path = outcome.project_path.join(consts::PYPROJECT_MANIFEST);
        let content = fs_err::read_to_string(pyproject_path).unwrap();
        assert!(
            content.contains("[tool.pixi.workspace]"),
            "Pyproject.toml should include [tool.pixi.workspace] after extending"
        )
    }

    #[rstest]
    #[tokio::test]

    async fn test_prompt_for_extending_pyproject_confirm_no(
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject: true,
            pre_existing_mojo,
            confirm_response: false,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(outcome.pyproject_exists);
        assert!(outcome.pixi_exists);

        let pyproject_path = outcome.project_path.join(consts::PYPROJECT_MANIFEST);
        let content = fs_err::read_to_string(pyproject_path).unwrap();
        assert!(
            !content.contains("[tool.pixi.workspace]"),
            "Pyproject.toml shouldn't include [tool.pixi.workspace] as extending didn't take place"
        )
    }

    #[rstest]
    #[tokio::test]

    async fn test_prompt_for_extending_pyproject_confirm_yes_already_extended(
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject: true,
            pre_existing_mojo,
            confirm_response: true,
            pyproject_already_extended: true,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(outcome.pyproject_exists);

        let pyproject_path = outcome.project_path.join(consts::PYPROJECT_MANIFEST);
        let content = fs_err::read_to_string(pyproject_path).unwrap();

        let pixi_table_count = content.matches("[tool.pixi.workspace]").count();

        assert_eq!(
            pixi_table_count, 1,
            "The [tool.pixi.workspace] table was duplicated! Early exit failed."
        );
    }

    #[rstest]
    #[tokio::test]
    async fn test_new_pyproject_format(
        #[values(true, false)] pre_existing_mojo: bool,
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pixi: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Pyproject),
            with_env_file: false,
            pre_existing_pixi,
            pre_existing_pyproject: false,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(outcome.pyproject_exists);
        assert_eq!(outcome.pixi_exists, pre_existing_pixi);

        let pyproject_path = outcome.project_path.join(consts::PYPROJECT_MANIFEST);
        let content = fs_err::read_to_string(pyproject_path).unwrap();
        assert!(
            content.contains("[tool.pixi.workspace]"),
            "Pyproject.toml should include [tool.pixi.workspace]"
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_mojo_format_fails_if_mojo_exists(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pixi: bool,
        #[values(true, false)] pre_existing_pyproject: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Mojoproject),
            with_env_file: false,
            pre_existing_pixi,
            pre_existing_pyproject,
            pre_existing_mojo: true,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        let error = outcome.result.unwrap_err();
        let error_msg = format!("{:?}", error);

        assert!(
            error_msg.contains("mojoproject.toml already exists"),
            "The command failed, but for the wrong reason. Error texts was: {}",
            error_msg
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_mojo_format_succeeds(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pixi: bool,
        #[values(true, false)] pre_existing_pyproject: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Mojoproject),
            with_env_file: false,
            pre_existing_pixi,
            pre_existing_pyproject,
            pre_existing_mojo: false,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.pyproject_exists, pre_existing_pyproject);
        assert_eq!(outcome.pixi_exists, pre_existing_pixi);
        assert!(outcome.mojo_exists);
    }

    #[rstest]
    #[tokio::test]
    async fn test_pixi_format_succeeds(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pyproject: bool,
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Pixi),
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.pyproject_exists, pre_existing_pyproject);
        assert_eq!(outcome.mojo_exists, pre_existing_mojo);
        assert!(outcome.pixi_exists);
    }

    #[rstest]
    #[tokio::test]
    async fn test_pixi_format_fails_if_pixi_exists(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pyproject: bool,
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Pixi),
            with_env_file: false,
            pre_existing_pixi: true,
            pre_existing_pyproject,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        let error = outcome.result.unwrap_err();
        let error_msg = format!("{:?}", error);

        assert!(
            error_msg.contains("pixi.toml already exists"),
            "The command failed, but for the wrong reason. Error texts was: {}",
            error_msg
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_init_default_fails_if_pixi_exists(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: true,
            pre_existing_pyproject: false,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        let error = outcome.result.unwrap_err();
        let error_msg = format!("{:?}", error);

        assert!(
            error_msg.contains("pixi.toml already exists"),
            "The command failed, but for the wrong reason. Error texts was: {}",
            error_msg
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_default_format_succeeds(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject: false,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(!outcome.pyproject_exists);
        assert_eq!(outcome.mojo_exists, pre_existing_mojo);
        assert!(outcome.pixi_exists);
    }
}
