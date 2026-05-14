#!/bin/bash
# Bump version in all required files
# Usage: ./scripts/bump-version.sh 0.1.0

set -e

if [ -z "$1" ]; then
    CURRENT=$(grep -m1 '^version = ' pyproject.toml | sed 's/version = "\(.*\)"/\1/')
    echo "Current version: $CURRENT"
    echo "Usage: $0 <new-version>"
    echo "Example: $0 0.1.0"
    exit 1
fi

NEW_VERSION="$1"

# Update pyproject.toml
sed -i "s/^version = \".*\"/version = \"$NEW_VERSION\"/" pyproject.toml

# Update plugin.json
sed -i "s/\"version\": \".*\"/\"version\": \"$NEW_VERSION\"/" .claude-plugin/plugin.json

# Update marketplace.json
sed -i "s/\"version\": \".*\"/\"version\": \"$NEW_VERSION\"/" .claude-plugin/marketplace.json

echo "Updated version to $NEW_VERSION in:"
echo "  - pyproject.toml"
echo "  - .claude-plugin/plugin.json"
echo "  - .claude-plugin/marketplace.json"

# Verify
echo ""
echo "Verification:"
grep -n "version.*$NEW_VERSION" pyproject.toml .claude-plugin/*.json
