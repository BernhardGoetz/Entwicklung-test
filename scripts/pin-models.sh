#!/usr/bin/env bash
# Trägt für jedes Modell in src-tauri/src/ai/catalog.json die aktuelle
# Revision (Commit-SHA) des Hugging-Face-Repos sowie Name, Größe und SHA-256
# der passenden GGUF-Datei(en) ein – aus der Hugging-Face-API, nie von Hand:
#
#   GET https://huggingface.co/api/models/<repo>/revision/main   -> .sha
#   GET https://huggingface.co/api/models/<repo>/tree/<sha>      -> lfs.oid = SHA-256
#
#   scripts/pin-models.sh [--check | --verify]
#
# Ist ein Modell auf Hugging Face in Teile aufgeteilt
# (…-00001-of-00002.gguf), werden alle Teile eingetragen; llama.cpp lädt sie,
# wenn sie nebeneinander liegen, über den ersten Teil.
# --check: nur prüfen, ob alle Einträge vollständig gepinnt sind: Revision
# (40 Hex-Zeichen), mindestens eine Datei, je Datei ein einfacher Name auf
# .gguf, Größe > 0 und SHA-256 (64 Hex-Zeichen). Der Release-Workflow ruft
# das nach dem Pinnen auf und bricht sonst ab.
# --verify: die eingecheckten Pins gegen die Hugging-Face-API prüfen: für die
# gepinnte Revision müssen Dateinamen, Größen und SHA-256 exakt
# übereinstimmen (jede Abweichung: Fehler). Eine neuere Revision auf „main“
# wird nur gemeldet. So pinnt der Release nie selbst (kein Vertrauen beim
# ersten Abruf): Pins entstehen über den Workflow pin-models.yml als Pull
# Request, den Bernhard prüft und mergt (docs/RELEASE_QAS.md).
# CATALOG=<Pfad> wählt eine andere Katalogdatei, HF_API=<https-URL> eine
# andere API-Basis (Tests).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
catalog="${CATALOG:-$root/src-tauri/src/ai/catalog.json}"
command -v jq >/dev/null || {
  echo "jq wird benötigt." >&2
  exit 1
}

if [ "${1:-}" = "--check" ]; then
  if [ "$(jq '.models | length' "$catalog")" -eq 0 ]; then
    echo "catalog.json enthält keine Modelle." >&2
    exit 1
  fi
  unpinned="$(jq -r '.models[]
    | select(((.revision // "") | tostring | test("^[0-9a-f]{40}$") | not)
        or ((.files // []) | length) == 0
        or any(.files[];
            ((.sha256 // "") | tostring | test("^[0-9a-f]{64}$") | not)
            or ((.size | type) != "number") or .size <= 0
            or ((.name // "") | tostring | test("^[^/\\\\:.][^/\\\\:]*\\.gguf$"; "i") | not)))
    | .id' "$catalog" | tr '\n' ' ')"
  if [ -n "$unpinned" ]; then
    echo "Nicht gepinnte Modelle in catalog.json: $unpinned (scripts/pin-models.sh ausführen)" >&2
    exit 1
  fi
  echo "Alle Modelle gepinnt."
  exit 0
fi

api="${HF_API:-https://huggingface.co/api/models}"
case "$api" in
  https://*) ;;
  *)
    echo "HF_API muss mit https:// beginnen." >&2
    exit 1
    ;;
esac

get() {
  curl -fsSL --proto '=https' --tlsv1.2 --retry 3 "$1"
}

# GGUF-Dateien der Quantisierung $3 in Revision $2 von Repo $1, nach Namen
# sortiert: [{name, size, sha256}].
files_at() {
  get "$api/$1/tree/$2" | jq --arg q "$3" '
    [ .[]
      | select(.type == "file")
      | select(.path | ascii_downcase | test("^[^/]*-" + $q + "(-[0-9]{5}-of-[0-9]{5})?\\.gguf$"))
      | { name: .path, size: (.lfs.size // .size), sha256: .lfs.oid } ]
    | sort_by(.name)'
}

if [ "${1:-}" = "--verify" ]; then
  failed=0
  for id in $(jq -r '.models[].id' "$catalog"); do
    m="$(jq -c --arg id "$id" '.models[] | select(.id == $id)' "$catalog")"
    repo="$(jq -r .repo <<<"$m")"
    quant="$(jq -r .quant <<<"$m")"
    rev="$(jq -r .revision <<<"$m")"
    if ! [[ "$rev" =~ ^[0-9a-f]{40}$ ]]; then
      echo "$id: nicht gepinnt (scripts/pin-models.sh --check)." >&2
      failed=1
      continue
    fi
    pinned="$(jq -S '.files | sort_by(.name) | map({name, size, sha256})' <<<"$m")"
    if ! current="$(files_at "$repo" "$rev" "$quant" | jq -S .)"; then
      echo "::error::$id: $repo@$rev nicht abrufbar (Revision gelöscht?)." >&2
      failed=1
      continue
    fi
    if [ "$pinned" != "$current" ]; then
      echo "::error::$id: Hugging Face meldet für $repo@$rev andere Dateien als catalog.json." >&2
      diff <(echo "$pinned") <(echo "$current") >&2 || true
      failed=1
      continue
    fi
    echo "$id: $repo@$rev stimmt mit catalog.json überein."
    latest="$(get "$api/$repo/revision/main" | jq -r '.sha // empty' || true)"
    if [ -n "$latest" ] && [ "$latest" != "$rev" ]; then
      echo "::notice::$id: auf main gibt es eine neuere Revision ($latest); gepinnt bleibt $rev."
    fi
  done
  exit "$failed"
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
cp "$catalog" "$tmp"

for id in $(jq -r '.models[].id' "$catalog"); do
  repo="$(jq -r --arg id "$id" '.models[] | select(.id == $id) | .repo' "$catalog")"
  quant="$(jq -r --arg id "$id" '.models[] | select(.id == $id) | .quant' "$catalog")"
  rev="$(get "$api/$repo/revision/main" | jq -r .sha)"
  if ! [[ "$rev" =~ ^[0-9a-f]{40}$ ]]; then
    echo "$repo: keine Revision gefunden." >&2
    exit 1
  fi
  files="$(files_at "$repo" "$rev" "$quant")"
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
