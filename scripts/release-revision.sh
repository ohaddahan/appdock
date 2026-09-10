#!/usr/bin/env bash
# Resolve the checked-out Cargo version and immutable commit. CI supplies event
# metadata; only an explicit manual release creates a missing tag.
set -euo pipefail

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

if [[ "$EVENT_NAME" == push && "$REF_TYPE" == branch ]]; then
  printf 'sha=%s\ntag=\n' "$sha" >> "$GITHUB_OUTPUT"
  exit 0
fi
if [[ "$EVENT_NAME" != workflow_dispatch && "$EVENT_TAG" != "$tag" ]]; then
  echo "::error::Tag $EVENT_TAG must match Cargo.toml version $tag"
  exit 1
fi
if git show-ref --verify --quiet "refs/tags/$tag"; then
  if [[ "$(git rev-parse "refs/tags/$tag^{commit}")" != "$sha" ]]; then
    echo "::error::Existing tag $tag points to another commit; select that tag to retry or bump Cargo.toml."
    exit 1
  fi
elif [[ "$EVENT_NAME" == workflow_dispatch ]]; then
  gh api --method POST "repos/$GITHUB_REPOSITORY/git/refs" \
    -f "ref=refs/tags/$tag" -f "sha=$sha"
else
  echo "::error::Release tag $tag does not exist"
  exit 1
fi
printf 'tag=%s\nsha=%s\n' "$tag" "$sha" >> "$GITHUB_OUTPUT"
