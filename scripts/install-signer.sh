#!/usr/bin/env bash
# Installiert nur die Tauri-CLI (für `tauri signer sign`) in <ordner>, ohne
# die übrigen npm-Abhängigkeiten des Projekts und ohne Install-Skripte – für
# den Job updater-sign, in dem der private Updater-Schlüssel verfügbar ist.
#
#   scripts/install-signer.sh <ordner>
#   <ordner>/node_modules/.bin/tauri signer sign …
#
# Version und Integrität (sha512) stammen aus der eingecheckten
# package-lock.json: daraus wird ein Lockfile nur mit @tauri-apps/cli und
# seinen Plattformpaketen erzeugt; `npm ci` prüft die Integrität jedes
# Pakets und bricht bei Abweichung ab.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:?Aufruf: $0 <ordner>}"
command -v jq >/dev/null || {
  echo "jq wird benötigt." >&2
  exit 1
}

version="$(jq -r '.packages["node_modules/@tauri-apps/cli"].version // empty' "$root/package-lock.json")"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "@tauri-apps/cli fehlt in package-lock.json." >&2
  exit 1
fi

mkdir -p "$out"
jq -n --arg v "$version" \
  '{name: "updater-signer", version: "0.0.0", private: true, dependencies: {"@tauri-apps/cli": $v}}' \
  >"$out/package.json"
jq --arg v "$version" '
  {
    name: "updater-signer",
    version: "0.0.0",
    lockfileVersion: 3,
    requires: true,
    packages: (
      {"": {name: "updater-signer", version: "0.0.0", dependencies: {"@tauri-apps/cli": $v}}}
      + (.packages
         | with_entries(select(.key | test("^node_modules/@tauri-apps/cli(-[a-z0-9-]+)?$")))
         | map_values(del(.dev)))
    )
  }' "$root/package-lock.json" >"$out/package-lock.json"

if jq -e '.packages | to_entries[] | select(.key != "") | select((.value.integrity // "") | startswith("sha512-") | not)' \
  "$out/package-lock.json" >/dev/null; then
  echo "Integritätswert fehlt im Lockfile." >&2
  exit 1
fi

(cd "$out" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error)
"$out/node_modules/.bin/tauri" --version
