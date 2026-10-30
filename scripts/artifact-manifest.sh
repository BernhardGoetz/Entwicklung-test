#!/usr/bin/env bash
# Bindet Release-Artefakte über Job-Grenzen an das, was der erzeugende Job
# hochgeladen hat (docs/RELEASE_QAS.md, „Integrität der Artefakte“).
#
# Jeder Schritt eines späteren Jobs kann mit ACTIONS_RUNTIME_TOKEN Artefakte
# desselben Laufs ersetzen (upload-artifact mit overwrite). Deshalb gibt der
# erzeugende Job ein Manifest („<sha256>  <dateiname>“ je Datei) als
# Job-Output weiter, und jeder Job, der ein Artefakt verwendet, prüft es vor
# der Verwendung gegen dieses Manifest.
#
#   scripts/artifact-manifest.sh write <output-name> <datei>...
#       Manifest der Dateien (Dateinamen ohne Pfad, wie upload-artifact sie
#       ablegt) nach $GITHUB_OUTPUT (mehrzeilig) und auf stdout.
#   scripts/artifact-manifest.sh verify <ordner> <manifest>...
#       Alle Dateien unter <ordner> (rekursiv, nur Dateinamen) müssen genau
#       den angegebenen Manifesten entsprechen: gleiche Namen, gleiche
#       SHA-256, keine zusätzlichen und keine fehlenden Dateien. Ein leeres
#       oder ungültiges Manifest (Job-Output fehlt) ist ein Fehler.
set -euo pipefail

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum -- "$1" | cut -d' ' -f1
  else
    shasum -a 256 -- "$1" | cut -d' ' -f1
  fi
}

manifest() {
  local f
  for f in "$@"; do
    printf '%s  %s\n' "$(sha256_of "$f")" "$(basename -- "$f")"
  done | LC_ALL=C sort
}

die() {
  echo "::error::$*" >&2
  exit 1
}

case "${1:-}" in
  write)
    [ "$#" -ge 3 ] || die "Aufruf: $0 write <output-name> <datei>..."
    name="$2"
    shift 2
    for f in "$@"; do
      [ -f "$f" ] || die "$f fehlt."
    done
    m="$(manifest "$@")"
    printf '%s\n' "$m"
    if [ -n "${GITHUB_OUTPUT:-}" ]; then
      delim="EOF_$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')"
      {
        echo "$name<<$delim"
        printf '%s\n' "$m"
        echo "$delim"
      } >>"$GITHUB_OUTPUT"
    fi
    ;;
  verify)
    [ "$#" -ge 3 ] || die "Aufruf: $0 verify <ordner> <manifest>..."
    dir="$2"
    shift 2
    [ -d "$dir" ] || die "$dir fehlt."
    expected="$(printf '%s\n' "$@" | sed '/^[[:space:]]*$/d' | LC_ALL=C sort)"
    [ -n "$expected" ] || die "Keine Prüfsummen für $dir (Job-Output fehlt)."
    for m in "$@"; do
      [ -n "$(printf '%s' "$m" | tr -d '[:space:]')" ] || die "Leeres Manifest für $dir (Job-Output fehlt)."
    done
    if printf '%s\n' "$expected" | grep -Evq '^[0-9a-f]{64}  [^/]+$'; then
      die "Ungültiges Manifest für $dir."
    fi
    files=()
    while IFS= read -r -d '' f; do files+=("$f"); done < <(find "$dir" -type f -print0)
    [ "${#files[@]}" -gt 0 ] || die "$dir ist leer."
    actual="$(manifest "${files[@]}")"
    if [ "$actual" != "$expected" ]; then
      echo "Erwartet:" >&2
      printf '%s\n' "$expected" >&2
      echo "Vorgefunden:" >&2
      printf '%s\n' "$actual" >&2
      die "Artefakt in $dir weicht vom Manifest des erzeugenden Jobs ab (ersetzt oder verändert?)."
    fi
    echo "$dir: ${#files[@]} Datei(en) geprüft."
    ;;
  *)
    die "Aufruf: $0 write <output-name> <datei>... | verify <ordner> <manifest>..."
    ;;
esac
