#!/usr/bin/env bash
# Bump every version in the workspace, and regenerate Cargo.lock as part of
# the same operation.
#
# This exists because the lock step is the one that gets forgotten. Bumping
# the manifests by hand and committing is enough to break `cargo publish
# --locked`, which is the *first* command in the pipeline that sees the lock
# as committed -- CI does not save you, because any unlocked cargo command
# there rewrites the lock on disk before the check runs, and the publish
# workflow runs off the tag in parallel with CI anyway. It broke v0.1.6 and
# then broke v0.1.8 the same way. So: one command, no order to remember.
#
#   scripts/bump-version.sh 0.1.9
set -euo pipefail

NEW="${1:?usage: bump-version.sh <new-version>}"
cd "$(dirname "$0")/.."

OLD=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
[ "$OLD" = "$NEW" ] && { echo "already at $NEW"; exit 1; }
echo "==> $OLD -> $NEW"

python3 - "$OLD" "$NEW" <<'PY'
import json, pathlib, sys
old, new = sys.argv[1], sys.argv[2]
p = pathlib.Path("Cargo.toml")
p.write_text(p.read_text().replace(f'version = "{old}"', f'version = "{new}"'))
p = pathlib.Path("bindings/node/package.json")
d = json.loads(p.read_text()); d["version"] = new
for k in d.get("optionalDependencies", {}):
    d["optionalDependencies"][k] = new
p.write_text(json.dumps(d, indent=2) + "\n")
for pkg in pathlib.Path("bindings/node/npm").glob("*/package.json"):
    d = json.loads(pkg.read_text()); d["version"] = new
    pkg.write_text(json.dumps(d, indent=2) + "\n")
p = pathlib.Path("bindings/python/pyproject.toml")
p.write_text(p.read_text().replace(f'version = "{old}"', f'version = "{new}"'))
print("    manifests updated")
PY

# The whole point of this script.
cargo metadata --format-version 1 > /dev/null
echo "    Cargo.lock regenerated"

# Prove it, the same way `cargo publish` will.
cargo metadata --locked --format-version 1 > /dev/null
echo "    verified: cargo is satisfied with the committed lock"

echo "==> done. Review, commit, then tag v$NEW"
