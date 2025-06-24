from importlib.metadata import version
from importlib.util import find_spec


def test_flask():
    # Don't test version, as it may change over time with lock file updates
    assert find_spec("flask") is not None


def test_rich():
    assert version("rich").split(".")[0] == "13"


def test_httpx():
    assert version("httpx") == "0.28.1"


def test_minimal_project():
    assert version("minimal_project") == "0.1"


def test_click():
    assert version("click") == "8.1.7"
