#!/usr/bin/env bash
# Exportiert die Runner-Images als gzip-komprimierte `docker save`-Archive,
# die Release-Builds im Installationspaket mitliefern
# (src-tauri/tauri.release.conf.json -> <Ressourcenordner>/images/).
#
#   scripts/export-image-archives.sh <amd64|arm64> [Zielordner]
#
# Umgebungsvariablen:
#   MODE=build (Standard)  Images lokal aus images/<sprache> bauen
#   MODE=pull              das veröffentlichte Image per Digest von GHCR holen
#                          (Release-Workflow: identische Digests wie online);
#                          benötigt IMAGE_DIGEST und genau eine Sprache
#   IMAGE_DIGEST=sha256:…  Digest (Manifest-Liste) des gepushten Images
#   LANGUAGES="python java"
#   IMAGE_REGISTRY=ghcr.io/<owner>  (Standard: IMAGE_REGISTRY aus languages.rs)
#
# Das Archiv enthält die Referenz <IMAGE_REGISTRY>/code-pruefer-<sprache>:<version>,
# die die App erwartet (Version = src-tauri/Cargo.toml).
#
# Zusätzlich wird <Zielordner>/image-digests.json geschrieben bzw. ergänzt:
# je Sprache und Architektur die SHA-256 des Archivs und die Image-IDs
# (Manifest- und Config-Digest) daraus, mit MODE=pull außerdem der
# Registry-Digest. Die App prüft geladene Archive und Downloads dagegen
# (src-tauri/src/runner/images.rs).
set -euo pipefail

arch="${1:-}"
case "$arch" in
  amd64 | arm64) ;;
  *)
    echo "Aufruf: $0 <amd64|arm64> [Zielordner]" >&2
    exit 2
    ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${2:-$root/src-tauri/resources/images}"
mode="${MODE:-build}"
languages="${LANGUAGES:-python java}"
digest="${IMAGE_DIGEST:-}"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/src-tauri/Cargo.toml" | head -n 1)"
registry="${IMAGE_REGISTRY:-$(sed -n 's/^pub const IMAGE_REGISTRY: &str = "\(.*\)";$/\1/p' \
  "$root/src-tauri/src/runner/languages.rs")}"
if [ -z "$version" ] || [ -z "$registry" ]; then
  echo "Version oder Registry nicht gefunden." >&2
  exit 1
fi
if [ "$mode" = pull ]; then
  if ! [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "MODE=pull benötigt IMAGE_DIGEST=sha256:<64 Hex-Zeichen>." >&2
    exit 2
  fi
  if [ "$(wc -w <<<"$languages")" -ne 1 ]; then
    echo "MODE=pull exportiert genau eine Sprache (LANGUAGES)." >&2
    exit 2
  fi
fi
command -v jq >/dev/null || {
  echo "jq wird benötigt." >&2
  exit 1
}

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# Image-IDs, unter denen eine Engine das Archiv führt: Manifest-Digest
# (Docker mit containerd-Image-Store) bzw. Config-Digest (klassischer
# Docker-Store, Podman).
image_ids_of() {
  {
    tar -xzOf "$1" index.json 2>/dev/null | jq -r '.manifests[].digest' || true
    tar -xzOf "$1" manifest.json | jq -r '.[].Config' |
      sed -E 's#^blobs/sha256/#sha256:#; s#^([0-9a-f]{64})\.json$#sha256:\1#'
  } | grep -E '^sha256:[0-9a-f]{64}$' | jq -R . | jq -cs 'unique'
}

mkdir -p "$out"
digests="$out/image-digests.json"
# Eine Datei einer anderen Version (oder kaputte Datei) wird ersetzt.
if [ -f "$digests" ] && ! jq -e --arg v "$version" '.version == $v' "$digests" >/dev/null 2>&1; then
  rm -f "$digests"
fi

for lang in $languages; do
  repo="$registry/code-pruefer-$lang"
  ref="$repo:$version"
  file="$out/code-pruefer-$lang-$arch.tar.gz"
  echo "==> $ref ($arch) -> $file"
  if [ "$mode" = pull ]; then
    docker pull --platform "linux/$arch" "$repo@$digest"
    docker tag "$repo@$digest" "$ref"
  else
    docker build --platform "linux/$arch" -t "$ref" "$root/images/$lang"
  fi
  docker save --platform "linux/$arch" "$ref" | gzip -6 > "$file.tmp"
  # Plausibilitätsprüfung: Archiv lesbar und mit der erwarteten Referenz.
  tar -xzOf "$file.tmp" manifest.json | grep -q "\"$ref\"" || {
    echo "Archiv $file enthält $ref nicht." >&2
    exit 1
  }
  ids="$(image_ids_of "$file.tmp")"
  if [ "$ids" = "[]" ]; then
    echo "Keine Image-ID in $file gefunden." >&2
    exit 1
  fi
  mv "$file.tmp" "$file"
  ls -l "$file"

  entry="$(jq -n --arg file "$(basename "$file")" --arg sha "$(sha256_of "$file")" \
    --argjson ids "$ids" '{file: $file, sha256: $sha, imageIds: $ids}')"
  new="$(jq -n --arg v "$version" --arg reg "$registry" --arg lang "$lang" \
    --arg arch "$arch" --argjson e "$entry" \
    '{version: $v, registry: $reg, images: {($lang): {archives: {($arch): $e}}}}')"
  [ -f "$digests" ] || echo '{}' > "$digests"
  # Zusammenführen; ohne MODE=pull gibt es keinen (gültigen) Registry-Digest.
  jq -s --arg lang "$lang" --arg rd "$digest" --arg mode "$mode" '
    (.[0] * .[1])
    | if $mode == "pull" then .images[$lang].repoDigest = $rd
      else del(.images[$lang].repoDigest) end' \
    "$digests" <(printf '%s' "$new") > "$digests.tmp"
  mv "$digests.tmp" "$digests"
done
echo "==> $digests"
jq . "$digests"
