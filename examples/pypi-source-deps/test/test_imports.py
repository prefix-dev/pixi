from importlib.metadata import version

def test_flask():
    assert version("flask") == "3.1.0.dev0"

def test_rich():
    assert version("rich").split(".")[0] == "13"

def test_requests():
    assert version("requests") == "2.31.0"

def test_minimal_project():
    assert version("minimal_project") == "0.1"

def test_click():
    assert version("click") == "8.1.7"
