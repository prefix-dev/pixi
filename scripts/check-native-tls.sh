#!/bin/bash
# Check that a specific crate is not pulled in as a dependency
# Useful for verifying feature flag configurations (e.g., native-tls vs rustls)

set -euo pipefail

# =============================================================================
# Configuration - customize these for your repository
# =============================================================================

# Crate to check for (script fails if this crate is found in dependency tree)
FORBIDDEN_CRATE="rustls"

# Features to exclude when testing (space-separated)
# These features will NOT be enabled during the check
# Note: Auto-generated features for optional deps (dep:X) are automatically excluded
EXCLUDE_FEATURES="rustls-tls default s3 gcs"

# Packages to skip entirely (space-separated)
# Use this for packages that are known to require the forbidden crate
SKIP_PACKAGES="rattler_s3"

# =============================================================================
# Script logic - typically no changes needed below
# =============================================================================

# Build jq filter for excluded features
exclude_filter=""
for feat in $EXCLUDE_FEATURES; do
    if [ -n "$exclude_filter" ]; then
        exclude_filter="$exclude_filter and "
    fi
    exclude_filter="${exclude_filter}.key != \"$feat\""
done

# Get workspace metadata once
metadata=$(cargo metadata --no-deps --format-version 1)

# Get all workspace packages
packages=$(echo "$metadata" | jq -r '.packages[].name')

failed=0
checked=0
skipped=0

for package in $packages; do
    # Skip packages that are known to require the forbidden crate
    if [ -n "$SKIP_PACKAGES" ] && echo "$SKIP_PACKAGES" | grep -qw "$package"; then
        echo "SKIP: $package (known $FORBIDDEN_CRATE dependency)"
        ((++skipped))
        continue
    fi

    ((++checked))

    features=$(echo "$metadata" | jq -r --arg pkg "$package" '
        .packages[] | select(.name == $pkg) | .features | to_entries[] |
        select(.value != ["dep:\(.key)"]) |
        select('"$exclude_filter"') |
        .key
    ' | tr '\n' ',' | sed 's/,$//')

    # Run cargo tree with all features except excluded features (prod dependencies only)
    if [ -n "$features" ]; then
        output=$(cargo tree -i "$FORBIDDEN_CRATE" --no-default-features --features "$features" --package "$package" --locked --edges=normal 2>&1 || true)
    else
        output=$(cargo tree -i "$FORBIDDEN_CRATE" --no-default-features --package "$package" --locked --edges=normal 2>&1 || true)
    fi

    if echo "$output" | grep -q "^$FORBIDDEN_CRATE"; then
        echo "FAIL: $package has $FORBIDDEN_CRATE dependency"
        if [ -n "$features" ]; then
            echo "Reproduce: cargo tree -i $FORBIDDEN_CRATE --no-default-features --features \"$features\" --package \"$package\" --locked --edges=normal"
        else
            echo "Reproduce: cargo tree -i $FORBIDDEN_CRATE --no-default-features --package \"$package\" --locked --edges=normal"
        fi
        echo "$output" | head -20
        echo ""
        ((++failed))
    else
        echo "OK:   $package"
    fi
done

echo ""
echo "Summary: $checked checked, $failed failed, $skipped skipped"

if [ $failed -gt 0 ]; then
    exit 1
fi
