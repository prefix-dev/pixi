import os
import tomllib

from pathlib import Path

from dirty_equals import IsPartialDict
from inline_snapshot import snapshot
import yaml

from .common import (
    ExitCode,
    verify_cli_command,
)


class TestCondaEnv:
    simple_env_yaml = {
        "name": "simple-env",
        "channels": ["conda-forge"],
        "dependencies": ["python"],
    }

    cowpy_env_yaml = {
        "name": "cowpy",
        "channels": ["conda-forge"],
        "dependencies": ["cowpy"],
    }

    noname_env_yaml = {
        "channels": ["conda-forge"],
        "dependencies": ["python"],
    }

    xpx_env_yaml = {
        "name": "array-api-extra",
        "channels": ["conda-forge"],
        "dependencies": ["array-api-extra"],
    }

    complex_env_yaml = {
        "name": "complex-env",
        "channels": ["conda-forge", "bioconda"],
        "dependencies": ["cowpy=1.1.4", "libblas=*=*openblas", "snakemake-minimal"],
    }

    def test_import_invalid_format(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"

        import_file_path = tmp_pixi_workspace / "simple_environment.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.simple_env_yaml, file)

        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # try to import as an invalid format
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
                "--format=foobar",
            ],
            ExitCode.INCORRECT_USAGE,
            stderr_contains="invalid value 'foobar' for '--format <FORMAT>'",
        )

    def test_import_conda_env(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"

        import_file_path = tmp_pixi_workspace / "simple_environment.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.simple_env_yaml, file)

        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # Import a simple `environment.yml`
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
                "--format=conda-env",
            ],
        )

        # check that no environments are installed
        assert not os.path.isdir(tmp_pixi_workspace / ".pixi/envs")

        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert "python" in parsed_manifest["feature"]["simple-env"]["dependencies"]
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "simple-env": {
                        "channels": ["conda-forge"],
                        "dependencies": {"python": "*"},
                    }
                },
                "environments": {
                    "simple-env": {"features": ["simple-env"], "no-default-feature": True}
                },
            }
        )

    def test_import_no_format(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"

        import_file_path = tmp_pixi_workspace / "simple_environment.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.simple_env_yaml, file)

        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # Import a simple `environment.yml` without specifying `format`
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
            ],
        )

        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert "python" in parsed_manifest["feature"]["simple-env"]["dependencies"]
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "simple-env": {
                        "channels": ["conda-forge"],
                        "dependencies": {"python": "*"},
                    }
                },
                "environments": {
                    "simple-env": {"features": ["simple-env"], "no-default-feature": True}
                },
            }
        )

    def test_import_no_name(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"

        import_file_path = tmp_pixi_workspace / "noname.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.noname_env_yaml, file)

        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # Import an `environment.yml` without a name
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
            ],
            ExitCode.FAILURE,
            stderr_contains="Missing name: provide --feature or --environment, or set `name:`",
        )

        # Providing a feature name succeeds
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
                "--feature=foobar",
            ],
        )

    def test_import_platforms(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"

        import_file_path = tmp_pixi_workspace / "simple_environment.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.simple_env_yaml, file)

        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # Import a simple `environment.yml` for linux-64 only
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
                "--platform=linux-64",
            ],
        )

        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert (
            "python"
            in parsed_manifest["feature"]["simple-env"]["target"]["linux-64"]["dependencies"]
        )
        assert "osx-arm64" not in parsed_manifest["feature"]["simple-env"]["target"]
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "simple-env": {
                        "platforms": ["linux-64"],
                        "channels": ["conda-forge"],
                        "target": {"linux-64": {"dependencies": {"python": "*"}}},
                    }
                },
                "environments": {
                    "simple-env": {"features": ["simple-env"], "no-default-feature": True}
                },
            }
        )

    def test_import_feature_environment(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"

        import_file_path = tmp_pixi_workspace / "simple_environment.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.simple_env_yaml, file)

        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # by default, a new env and feature are created with the name of the imported file,
        # with no-default-feature: True
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
            ],
        )
        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert "simple-env" in parsed_manifest["environments"]["simple-env"]["features"]
        assert parsed_manifest["environments"]["simple-env"]["no-default-feature"] is True
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "simple-env": {
                        "channels": ["conda-forge"],
                        "dependencies": {"python": "*"},
                    }
                },
                "environments": {
                    "simple-env": {"features": ["simple-env"], "no-default-feature": True}
                },
            }
        )

        # we can import into an existing feature
        import_file_path = tmp_pixi_workspace / "cowpy.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.cowpy_env_yaml, file)

        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
                "--feature=simple-env",
            ],
        )
        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert "cowpy" in parsed_manifest["feature"]["simple-env"]["dependencies"]
        assert "cowpy" not in parsed_manifest["environments"]
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "simple-env": {
                        "channels": ["conda-forge"],
                        "dependencies": {"python": "*", "cowpy": "*"},
                    }
                },
                "environments": {
                    "simple-env": {"features": ["simple-env"], "no-default-feature": True}
                },
            }
        )

        # we can create a new feature and add it to an existing environment
        import_file_path = tmp_pixi_workspace / "array-api-extra.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.xpx_env_yaml, file)

        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
                "--environment=simple-env",
                "--feature=array-api-extra",
            ],
        )
        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert "array-api-extra" in parsed_manifest["feature"]["array-api-extra"]["dependencies"]
        assert "array-api-extra" in parsed_manifest["environments"]["simple-env"]["features"]
        # no new environment should be created
        assert "array-api-extra" not in parsed_manifest["environments"]
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "simple-env": {
                        "channels": ["conda-forge"],
                        "dependencies": {"python": "*", "cowpy": "*"},
                    },
                    "array-api-extra": {
                        "channels": ["conda-forge"],
                        "dependencies": {"array-api-extra": "*"},
                    },
                },
                "environments": {
                    "simple-env": {
                        "features": ["simple-env", "array-api-extra"],
                        "no-default-feature": True,
                    }
                },
            }
        )

        # we can create a new feature (and a matching env by default)
        import_file_path = tmp_pixi_workspace / "cowpy.yml"
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
                "--feature=farm",
            ],
        )
        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert "farm" in parsed_manifest["environments"]["farm"]["features"]
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "simple-env": {
                        "channels": ["conda-forge"],
                        "dependencies": {"python": "*", "cowpy": "*"},
                    },
                    "array-api-extra": {
                        "channels": ["conda-forge"],
                        "dependencies": {"array-api-extra": "*"},
                    },
                    "farm": {"channels": ["conda-forge"], "dependencies": {"cowpy": "*"}},
                },
                "environments": {
                    "simple-env": {
                        "features": ["simple-env", "array-api-extra"],
                        "no-default-feature": True,
                    },
                    "farm": {"features": ["farm"], "no-default-feature": True},
                },
            }
        )

        # we can create a new env (and a matching feature by default)
        import_file_path = tmp_pixi_workspace / "array-api-extra.yml"
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
                "--feature=data",
            ],
        )
        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert "data" in parsed_manifest["environments"]["data"]["features"]
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "simple-env": {
                        "channels": ["conda-forge"],
                        "dependencies": {"python": "*", "cowpy": "*"},
                    },
                    "array-api-extra": {
                        "channels": ["conda-forge"],
                        "dependencies": {"array-api-extra": "*"},
                    },
                    "farm": {"channels": ["conda-forge"], "dependencies": {"cowpy": "*"}},
                    "data": {"channels": ["conda-forge"], "dependencies": {"array-api-extra": "*"}},
                },
                "environments": {
                    "simple-env": {
                        "features": ["simple-env", "array-api-extra"],
                        "no-default-feature": True,
                    },
                    "farm": {"features": ["farm"], "no-default-feature": True},
                    "data": {"features": ["data"], "no-default-feature": True},
                },
            }
        )

    def test_import_channels_and_versions(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"

        import_file_path = tmp_pixi_workspace / "complex_environment.yml"
        with open(import_file_path, "w") as file:
            yaml.dump(self.complex_env_yaml, file)

        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # Import an environment which uses bioconda, pins versions, and specifies a variant
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                import_file_path,
            ],
        )
        parsed_manifest = tomllib.loads(manifest_path.read_text())
        assert parsed_manifest == snapshot(
            {
                # these keys are irrelevant and some are machine-dependent
                "workspace": IsPartialDict,
                "tasks": {},
                "dependencies": {},
                "feature": {
                    "complex-env": {
                        "channels": ["conda-forge", "bioconda"],
                        "dependencies": {
                            "cowpy": "1.1.4.*",
                            "libblas": {"version": "*", "build": "*openblas"},
                            "snakemake-minimal": "*",
                        },
                    }
                },
                "environments": {
                    "complex-env": {"features": ["complex-env"], "no-default-feature": True}
                },
            }
        )
