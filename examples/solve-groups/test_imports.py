import os
import sys
import pytest

def test_imports():
    if os.environ["PIXI_ENVIRONMENT_NAME"] == "min-py38":
        # importing pydantic is not possible in this environment
        with pytest.raises(ImportError):
            pass
        # Python version is higher than 3.8
        assert (3, 8) < sys.version_info < (3, 11), \
            "Python version is not between 3.8 and 3.10"

    if os.environ["PIXI_ENVIRONMENT_NAME"] == "max-py310":
        # importing py_rattler is not possible in this environment
        with pytest.raises(ImportError):
            pass
        # Python version is lower than 3.10 and higher than 3.8
        assert (3, 8) < sys.version_info < (3, 11), \
            "Python version is not between 3.8 and 3.10"
