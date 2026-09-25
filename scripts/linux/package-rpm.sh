#!/usr/bin/env bash
# Build an RPM from the staged tree (scripts/linux/stage.sh).
# Requires: rpmbuild.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
version="$(grep -m1 '^version' "$repo_root/Cargo.toml" | cut -d'"' -f2)"
rpm="$repo_root/build/tpt-focus-${version}-1.x86_64.rpm"

cd "$repo_root"

echo "==> rpmbuild"
rpmbuild -bb --define "_version $version" \
    --define "_rpmdir $repo_root/build" \
    --define "_sourcedir $repo_root" \
    --define "_specdir $repo_root/packaging/linux" \
    --define "_srcrpmdir $repo_root/build" \
    --define "_builddir $repo_root/build/rpmbuild" \
    packaging/linux/tpt-focus.spec

echo "built build/RPMS/x86_64/tpt-focus-${version}-1.x86_64.rpm"
[ -f "$rpm" ] || mv "$repo_root/build/x86_64/tpt-focus-${version}-1.x86_64.rpm" "$rpm" 2>/dev/null || true
echo "built $rpm"
