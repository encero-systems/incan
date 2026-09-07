"""Thin Python installer package for the Incan toolchain."""

__all__ = ["__release_version__", "__version__"]

# Python packages require PEP 440 versions, while toolchain release assets retain the
# workspace's Cargo version spelling. Keep both identities explicit so an installer
# never derives a GitHub release URL from a normalized distribution version.
__release_version__ = "0.6.0-dev.2"
__version__ = "0.6.0.dev2"
