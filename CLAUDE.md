# Version Management

## Single Source of Truth

**pyproject.toml** is the single source of truth for the package version.

- `cli.py` reads version dynamically via `importlib.metadata`
- Plugin JSON files (`.claude-plugin/*.json`) must be updated manually when bumping

## Bumping Version

```bash
./scripts/bump-version.sh 0.1.0
```

This updates all version locations:
- `pyproject.toml` (PyPI package)
- `.claude-plugin/plugin.json` (plugin metadata)
- `.claude-plugin/marketplace.json` (marketplace metadata)

## Pre-push Hook

A git pre-push hook validates that the version in pyproject.toml doesn't already exist on PyPI.
This prevents CI failures from trying to upload duplicate versions.

## Breaking Changes

Only bump minor version (0.1 → 0.2) for breaking changes. Update SKILL.md minimum version if needed.


# Testing

You can run tests using pytest:

```bash
pytest tests/ -v

pytest tests/<filename> # run specific test
```