#!/usr/bin/env bash
# Resolve the checked-out Cargo version and immutable commit. CI supplies event
# metadata; only an explicit manual release creates a missing tag.
set -euo pipefail

if [[ "$EVENT_NAME" != workflow_dispatch ]]; then
  echo '::error::Releases must be started manually with workflow_dispatch.'
  exit 1
fi

version="$(python3 - <<'PY'
import re, tomllib
with open('Cargo.toml', 'rb') as file:
    version = tomllib.load(file)['package']['version']
if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?', version):
    raise SystemExit('Cargo.toml must contain a release version')
print(version)
PY
)"
tag="v${version}"
git check-ref-format "refs/tags/$tag"
sha="$(git rev-parse HEAD)"

if git show-ref --verify --quiet "refs/tags/$tag"; then
  if [[ "$(git rev-parse "refs/tags/$tag^{commit}")" != "$sha" ]]; then
    echo "::error::Existing tag $tag points to another commit; select that tag to retry or bump Cargo.toml."
    exit 1
  fi
else
  gh api --method POST "repos/$GITHUB_REPOSITORY/git/refs" \
    -f "ref=refs/tags/$tag" -f "sha=$sha"
fi
printf 'tag=%s\nsha=%s\n' "$tag" "$sha" >> "$GITHUB_OUTPUT"
