"""Smoke-test a conda-channel wheel alongside a uv-installed PyPI package."""

from importlib.metadata import version

from boltons.iterutils import chunked
from flask import Flask

assert chunked([1, 2, 3], 2) == [[1, 2], [3]]

app = Flask(__name__)


@app.get("/")
def index():
    return {"chunks": chunked([1, 2, 3], 2)}


response = app.test_client().get("/")
assert response.status_code == 200
assert response.json == {"chunks": [[1, 2], [3]]}
print(f"boltons {version('boltons')} (conda wheel) + flask {version('flask')} (PyPI): OK")
