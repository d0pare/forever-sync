#!/usr/bin/env bash
# Usage: scripts/release.sh 0.2.0
# Bumps the version everywhere, commits, tags (signed) and pushes. The
# Release workflow then builds and publishes installers + update files.
set -euo pipefail

version="${1:-}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: $0 X.Y.Z" >&2; exit 1; }
cd "$(git rev-parse --show-toplevel)"

[[ "$(git branch --show-current)" == main ]] || { echo "not on main" >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo "working tree not clean" >&2; exit 1; }
git fetch -q origin
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || { echo "main differs from origin/main" >&2; exit 1; }
git rev-parse -q --verify "refs/tags/v$version" >/dev/null && { echo "tag v$version exists" >&2; exit 1; }

python3 - "$version" <<'PY'
import json, re, sys
v = sys.argv[1]
p = "Cargo.toml"; s = open(p).read()
s = re.sub(r'(\[workspace\.package\][^\[]*?\nversion = ")[^"]+(")', rf'\g<1>{v}\g<2>', s, count=1)
open(p, "w").write(s)
for p in ["apps/desktop/package.json", "apps/desktop/src-tauri/tauri.conf.json"]:
    c = json.load(open(p)); c["version"] = v
    open(p, "w").write(json.dumps(c, indent=2) + "\n")
PY

cargo update -q --workspace
cargo build -q -p foreversync-cli   # sanity check

git add Cargo.toml Cargo.lock apps/desktop/package.json apps/desktop/src-tauri/tauri.conf.json
git commit -q -m "Release v$version"
git tag -s "v$version" -m "Forever Sync v$version"
git push -q origin main "v$version"
echo "Pushed v$version. Watch the build: gh run watch \$(gh run list -w Release -L1 --json databaseId -q '.[0].databaseId')"
