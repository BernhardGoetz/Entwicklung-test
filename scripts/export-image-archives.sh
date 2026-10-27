#!/usr/bin/env bash
# Exportiert die Runner-Images als gzip-komprimierte `docker save`-Archive,
# die Release-Builds im Installationspaket mitliefern
# (src-tauri/tauri.release.conf.json -> <Ressourcenordner>/images/).
#
#   scripts/export-image-archives.sh <amd64|arm64> [Zielordner]
#
# Umgebungsvariablen:
#   MODE=build (Standard)  Images lokal aus images/<sprache> bauen
#   MODE=pull              die veröffentlichten Images von GHCR holen
#                          (Release-Workflow: identische Digests wie online)
#   LANGUAGES="python java"
#   IMAGE_REGISTRY=ghcr.io/<owner>  (Standard: IMAGE_REGISTRY aus languages.rs)
#
# Das Archiv enthält die Referenz <IMAGE_REGISTRY>/code-pruefer-<sprache>:<version>,
# die die App erwartet (Version = src-tauri/Cargo.toml).
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
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/src-tauri/Cargo.toml" | head -n 1)"
registry="${IMAGE_REGISTRY:-$(sed -n 's/^pub const IMAGE_REGISTRY: &str = "\(.*\)";$/\1/p' \
  "$root/src-tauri/src/runner/languages.rs")}"
if [ -z "$version" ] || [ -z "$registry" ]; then
  echo "Version oder Registry nicht gefunden." >&2
  exit 1
fi

mkdir -p "$out"
for lang in $languages; do
  ref="$registry/code-pruefer-$lang:$version"
  file="$out/code-pruefer-$lang-$arch.tar.gz"
  echo "==> $ref ($arch) -> $file"
  if [ "$mode" = pull ]; then
    docker pull --platform "linux/$arch" "$ref"
  else
    docker build --platform "linux/$arch" -t "$ref" "$root/images/$lang"
  fi
  docker save --platform "linux/$arch" "$ref" | gzip -6 > "$file.tmp"
  # Plausibilitätsprüfung: Archiv lesbar und mit der erwarteten Referenz.
  tar -xzOf "$file.tmp" manifest.json | grep -q "\"$ref\"" || {
    echo "Archiv $file enthält $ref nicht." >&2
    exit 1
  }
  mv "$file.tmp" "$file"
  ls -l "$file"
done
