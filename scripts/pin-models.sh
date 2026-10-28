#!/usr/bin/env bash
# Trägt für jedes Modell in src-tauri/src/ai/catalog.json die aktuelle
# Revision (Commit-SHA) des Hugging-Face-Repos sowie Name, Größe und SHA-256
# der passenden GGUF-Datei(en) ein – aus der Hugging-Face-API, nie von Hand:
#
#   GET https://huggingface.co/api/models/<repo>/revision/main   -> .sha
#   GET https://huggingface.co/api/models/<repo>/tree/<sha>      -> lfs.oid = SHA-256
#
#   scripts/pin-models.sh [--check]
#
# Ist ein Modell auf Hugging Face in Teile aufgeteilt
# (…-00001-of-00002.gguf), werden alle Teile eingetragen; llama.cpp lädt sie,
# wenn sie nebeneinander liegen, über den ersten Teil.
# --check: nur prüfen, ob alle Einträge gepinnt sind (Release-Workflow).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
catalog="$root/src-tauri/src/ai/catalog.json"
command -v jq >/dev/null || {
  echo "jq wird benötigt." >&2
  exit 1
}

if [ "${1:-}" = "--check" ]; then
  unpinned="$(jq -r '.models[]
    | select((.revision | test("^[0-9a-f]{40}$") | not)
        or (.files | length) == 0
        or any(.files[]; (.sha256 | test("^[0-9a-f]{64}$") | not) or .size <= 0))
    | .id' "$catalog" | tr '\n' ' ')"
  if [ -n "$unpinned" ]; then
    echo "Nicht gepinnte Modelle in catalog.json: $unpinned (scripts/pin-models.sh ausführen)" >&2
    exit 1
  fi
  echo "Alle Modelle gepinnt."
  exit 0
fi

api="https://huggingface.co/api/models"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
cp "$catalog" "$tmp"

for id in $(jq -r '.models[].id' "$catalog"); do
  repo="$(jq -r --arg id "$id" '.models[] | select(.id == $id) | .repo' "$catalog")"
  quant="$(jq -r --arg id "$id" '.models[] | select(.id == $id) | .quant' "$catalog")"
  rev="$(curl -fsSL --proto '=https' "$api/$repo/revision/main" | jq -r .sha)"
  if ! [[ "$rev" =~ ^[0-9a-f]{40}$ ]]; then
    echo "$repo: keine Revision gefunden." >&2
    exit 1
  fi
  files="$(curl -fsSL --proto '=https' "$api/$repo/tree/$rev" | jq --arg q "$quant" '
    [ .[]
      | select(.type == "file")
      | select(.path | ascii_downcase | test("^[^/]*-" + $q + "(-[0-9]{5}-of-[0-9]{5})?\\.gguf$"))
      | { name: .path, size: (.lfs.size // .size), sha256: .lfs.oid } ]
    | sort_by(.name)')"
  if [ "$(jq 'length' <<<"$files")" -eq 0 ] ||
    [ "$(jq '[.[] | select((.sha256 // "") | test("^[0-9a-f]{64}$") | not)] | length' <<<"$files")" -ne 0 ]; then
    echo "$repo@$rev: keine GGUF-Datei für $quant mit SHA-256 gefunden." >&2
    exit 1
  fi
  jq --arg id "$id" --arg rev "$rev" --argjson files "$files" \
    '(.models[] | select(.id == $id)) |= (.revision = $rev | .files = $files)' "$tmp" >"$tmp.new"
  mv "$tmp.new" "$tmp"
  echo "$id: $repo@$rev"
  jq -r '.[] | "  \(.name)  \(.size) Bytes  \(.sha256)"' <<<"$files"
done

cp "$tmp" "$catalog"
echo "catalog.json aktualisiert."
