"""Tests for automatic distro detection from robostack channels."""

import pytest

from pixi_build_ros.config import ROSBackendConfig, _extract_distro_from_channels_list


def test_extract_distro_from_full_url():
    """Test extracting distro from full robostack URL."""
    channels = [
        "https://prefix.dev/pixi-build-backends",
        "https://prefix.dev/robostack-jazzy",
        "https://prefix.dev/conda-forge",
    ]

    distro = _extract_distro_from_channels_list(channels)
    assert distro == "jazzy"


def test_extract_distro_from_short_channel_name():
    """Test extracting distro from short robostack channel name."""
    channels = ["robostack-humble", "conda-forge"]

    distro = _extract_distro_from_channels_list(channels)
    assert distro == "humble"


def test_dont_extract_from_staging():
    """Test extracting distro from short robostack channel name."""
    channels = ["robostack-staging", "conda-forge"]

    distro = _extract_distro_from_channels_list(channels)
    assert distro is None


def test_extract_distro_with_trailing_slash():
    """Test extracting distro from URL with trailing slash."""
    channels = ["https://prefix.dev/robostack-noetic/"]

    distro = _extract_distro_from_channels_list(channels)
    assert distro == "noetic"


def test_extract_distro_multiple_robostack_channels():
    """Test that the first robostack channel is used when multiple exist."""
    channels = [
        "https://prefix.dev/robostack-humble",
        "https://prefix.dev/robostack-jazzy",
    ]

    distro = _extract_distro_from_channels_list(channels)
    assert distro == "humble"


def test_extract_distro_no_robostack_channel():
    """Test that None is returned when no robostack channel exists."""
    channels = [
        "https://prefix.dev/conda-forge",
        "https://prefix.dev/some-other-channel",
    ]

    distro = _extract_distro_from_channels_list(channels)
    assert distro is None


def test_config_auto_detects_distro_from_channel():
    """Test that ROSBackendConfig auto-detects distro from channel list provided by the caller."""
    # Create config without distro specified
    config = ROSBackendConfig.model_validate({}, context={"channels": ["https://prefix.dev/robostack-jazzy"]})

    assert config.distro is not None
    assert config.distro.name == "jazzy"


def test_config_explicit_distro_overrides_channel():
    """Test that explicit distro config takes precedence over channel detection."""
    # Create config with explicit distro
    config = ROSBackendConfig.model_validate(
        {"distro": "humble"},
        context={"channels": ["https://prefix.dev/robostack-jazzy"]},
    )

    assert config.distro is not None
    assert config.distro.name == "humble"


def test_config_fails_without_distro_or_channel():
    """Test that config validation fails when distro cannot be determined."""
    with pytest.raises(ValueError, match="ROS distro must be either"):
        ROSBackendConfig.model_validate({}, context={"channels": ["conda-forge"]})


def test_config_fails_without_channels_context():
    """Test that config validation fails when no channels are provided."""
    with pytest.raises(ValueError, match="ROS distro must be either"):
        ROSBackendConfig.model_validate({})
