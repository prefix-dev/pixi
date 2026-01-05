"""Custom hatch metadata hook to provide dynamic dependencies."""

from __future__ import annotations

from typing import Any, ClassVar, override

from hatchling.metadata.plugin.interface import MetadataHookInterface


class CustomMetadataHook(MetadataHookInterface):
    """A custom metadata hook that provides dynamic dependencies."""

    PLUGIN_NAME: ClassVar[str] = "custom"

    @override
    def update(self, metadata: dict[str, Any]) -> None:
        """Update the metadata with dynamic dependencies."""
        # Add a simple dependency to test dynamic metadata extraction
        metadata["dependencies"] = ["typing-extensions>=4.0", "rich"]
