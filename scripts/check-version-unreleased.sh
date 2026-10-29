#!/usr/bin/env bash
# Prüft, dass eine Version noch nicht veröffentlicht ist (Release-Workflow,
# docs/RELEASE_QAS.md). Benötigt gh (mit GH_TOKEN), jq und GH_REPO.
#
#   scripts/check-version-unreleased.sh v<version>
#       Fehler, wenn der Tag oder ein Release dazu existiert.
#   scripts/check-version-unreleased.sh v<version> --rerun-ok <commit>
#       wie oben, erlaubt aber einen erneuten Lauf, der den Tag schon auf
#       <commit> gesetzt hat und dessen Release noch ein Entwurf ist (oder
#       fehlt). Schreibt dann tag_exists=true|false nach $GITHUB_OUTPUT.
#
# Entwurfs-Releases sieht nur ein Token mit Schreibrecht (contents: write).
set -euo pipefail

tag="${1:?Aufruf: $0 v<version> [--rerun-ok <commit>]}"
mode="${2:-}"
commit="${3:-}"
: "${GH_REPO:?GH_REPO fehlt}"

tag_commit=""
if out="$(gh api "repos/$GH_REPO/git/ref/tags/$tag" 2>&1)"; then
  type="$(jq -r .object.type <<<"$out")"
  sha="$(jq -r .object.sha <<<"$out")"
  if [ "$type" = "tag" ]; then
    sha="$(gh api "repos/$GH_REPO/git/tags/$sha" --jq .object.sha)"
  fi
  tag_commit="$sha"
elif ! grep -q "HTTP 404" <<<"$out"; then
  echo "::error::Tag $tag konnte nicht geprüft werden: $out" >&2
  exit 1
fi

release="$(gh api "repos/$GH_REPO/releases" --paginate \
  --jq ".[] | select(.tag_name == \"$tag\") | if .draft then \"draft\" else \"published\" end" | head -n 1)"

if [ "$mode" = "--rerun-ok" ]; then
  if [ -n "$tag_commit" ] && [ "$tag_commit" != "$commit" ]; then
    echo "::error::Tag $tag existiert bereits auf $tag_commit (nicht auf $commit)." >&2
    exit 1
  fi
  if [ "$release" = "published" ]; then
    echo "::error::Release $tag ist bereits veröffentlicht." >&2
    exit 1
  fi
  if [ -n "$tag_commit" ]; then
    echo "tag_exists=true" >>"${GITHUB_OUTPUT:-/dev/stdout}"
  else
    echo "tag_exists=false" >>"${GITHUB_OUTPUT:-/dev/stdout}"
  fi
  exit 0
fi

if [ -n "$tag_commit" ] || [ -n "$release" ]; then
  echo "::error::Version ${tag#v} ist bereits vergeben (Tag: ${tag_commit:-–}, Release: ${release:-–}). Version in Cargo.toml, tauri.conf.json und package.json erhöhen." >&2
  exit 1
fi
echo "$tag ist noch frei."
