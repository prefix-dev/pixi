from pathlib import Path

import pytest


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--pixi-build",
        action="store",
        default="release",
        help="Specify the pixi build type (e.g., release or debug)",
    )


@pytest.fixture
def pixi(request: pytest.FixtureRequest) -> Path:
    pixi_build = request.config.getoption("--pixi-build")
    return Path(__file__).parent.joinpath(f"../../target/pixi/{pixi_build}/pixi")


@pytest.fixture
def tmp_pixi_workspace(tmp_path: Path) -> Path:
    pixi_config = """
# Reset to defaults
default-channels = ["conda-forge"]
shell.change-ps1 = true
tls-no-verify = false
detached-environments = false
pinning-strategy = "semver"

[concurrency]
downloads = 50

[experimental]
use-environment-activation-cache = false

# Enable sharded repodata
[repodata-config."https://prefix.dev/"]
disable-sharded = false
"""
    dot_pixi = tmp_path.joinpath(".pixi")
    dot_pixi.mkdir()
    dot_pixi.joinpath("config.toml").write_text(pixi_config)
    return tmp_path


@pytest.fixture
def test_data() -> Path:
    return Path(__file__).parents[1].joinpath("data").resolve()


@pytest.fixture
def pypi_data(test_data: Path) -> Path:
    """
    Returns the pixi pypi test data
    """
    return test_data.joinpath("pypi")


@pytest.fixture
def pixi_tomls(test_data: Path) -> Path:
    """
    Returns the pixi pypi test data
    """
    return test_data.joinpath("pixi_tomls")


@pytest.fixture
def mock_projects(test_data: Path) -> Path:
    return test_data.joinpath("mock-projects")


@pytest.fixture
def channels(test_data: Path) -> Path:
    return test_data.joinpath("channels", "channels")


@pytest.fixture
def dummy_channel_1(channels: Path) -> str:
    return channels.joinpath("dummy_channel_1").as_uri()


@pytest.fixture
def dummy_channel_2(channels: Path) -> str:
    return channels.joinpath("dummy_channel_2").as_uri()


@pytest.fixture
def multiple_versions_channel_1(channels: Path) -> str:
    return channels.joinpath("multiple_versions_channel_1").as_uri()


@pytest.fixture
def non_self_expose_channel_1(channels: Path) -> str:
    return channels.joinpath("non_self_expose_channel_1").as_uri()


@pytest.fixture
def non_self_expose_channel_2(channels: Path) -> str:
    return channels.joinpath("non_self_expose_channel_2").as_uri()


@pytest.fixture
def virtual_packages_channel(channels: Path) -> str:
    return channels.joinpath("virtual_packages").as_uri()


@pytest.fixture
def shortcuts_channel_1(channels: Path) -> str:
    return channels.joinpath("shortcuts_channel_1").as_uri()


@pytest.fixture
def post_link_script_channel(channels: Path) -> str:
    return channels.joinpath("post_link_script_channel").as_uri()


def doc_pixi_workspaces() -> Path:
    return Path(__file__).parents[2].joinpath("docs", "source_files", "pixi_workspaces")
