#!/usr/bin/env bash
# Wertet eine Signaturrunde aus (scripts/signpath-stage.mjs,
# .github/workflows/release.yml): schreibt pending=true|false (es wurden
# Dateien zum Signieren gesammelt) und installer=true|false (darunter der
# Installer, also die letzte Runde) nach $GITHUB_OUTPUT und listet die Dateien.
# Mit --last ist es ein Fehler, wenn außer dem Installer noch etwas offen ist.
#
#   scripts/signpath-round.sh <stage-ordner> [--last]
set -euo pipefail

stage="${1:?Aufruf: $0 <stage-ordner> [--last]}"
last="${2:-}"
out="${GITHUB_OUTPUT:-/dev/stdout}"
pending=false
installer=false
if [ -d "$stage" ] && [ -n "$(ls -A "$stage")" ]; then
  pending=true
  echo "Zum Signieren gesammelt ($stage):"
  ls -l "$stage"
  if compgen -G "$stage/installer-*" >/dev/null; then
    installer=true
  fi
  if [ "$last" = "--last" ]; then
    for f in "$stage"/*; do
      case "$(basename "$f")" in
        installer-*) ;;
        *)
          echo "::error::Nach der letzten Runde ist noch $(basename "$f") unsigniert." >&2
          exit 1
          ;;
      esac
    done
  fi
else
  echo "Nichts mehr zu signieren."
fi
{
  echo "pending=$pending"
  echo "installer=$installer"
} >>"$out"
