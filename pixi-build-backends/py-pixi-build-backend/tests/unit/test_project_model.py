"""Unit tests for project_model.py module."""

from typing import Any
from pathlib import Path
import json
from pixi_build_backend.types.project_model import ProjectModel


def test_project_model_initialization(snapshot: Any) -> None:
    """Test initialization of ProjectModel."""
    model = ProjectModel(name="test_project", version="1.0.0")

    assert model._debug_str() == snapshot


def test_project_model_initialization_from_json(snapshot: Any) -> None:
    """Test initialization of ProjectModel."""
    model = ProjectModel.from_json(json.dumps({"name": "test_project", "version": "1.0.0"}))

    assert model._debug_str() == snapshot


def test_project_model_initialization_from_dict(snapshot: Any) -> None:
    """Test initialization of ProjectModel from a Python mapping."""
    model = ProjectModel.from_dict({"name": "test_project", "version": "1.0.0"})

    assert model._debug_str() == snapshot


def test_project_model_initialization_from_json_file(snapshot: Any) -> None:
    json_file = Path(__file__).parent.parent / "data" / "project_model_example.json"
    model = ProjectModel.from_json_file(json_file)
    assert model._debug_str() == snapshot
