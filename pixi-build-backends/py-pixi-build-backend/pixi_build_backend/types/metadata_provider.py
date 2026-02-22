"""
Python metadata provider types and protocols.
"""

from typing import Optional, Protocol


class MetadataProvider(Protocol):
    """
    Protocol for providing package metadata.

    This protocol defines the interface that metadata providers must implement
    to supply package metadata like name, version, description, etc.
    """

    def name(self) -> Optional[str]:
        """Return the package name or None if not available."""
        return None

    def version(self) -> Optional[str]:
        """Return the package version or None if not available."""
        return None

    def homepage(self) -> Optional[str]:
        """Return the package homepage URL or None if not available."""
        return None

    def license(self) -> Optional[str]:
        """Return the package license or None if not available."""
        return None

    def license_files(self) -> Optional[str]:
        """Return the path to the license file or None if not available."""
        return None

    def summary(self) -> Optional[str]:
        """Return a short package summary or None if not available."""
        return None

    def description(self) -> Optional[str]:
        """Return the package description or None if not available."""
        return None

    def documentation(self) -> Optional[str]:
        """Return the documentation URL or None if not available."""
        return None

    def repository(self) -> Optional[str]:
        """Return the repository URL or None if not available."""
        return None
