#!/usr/bin/env bash
# Schreibt die Release-Notizen (Markdown) nach stdout (Release-Workflow).
#
#   scripts/release-notes.sh <qas|release> <katalog.json>
#
# Umgebung: VERSION, REGISTRY, GH_REPO, IMAGE_TAG (Tag der Runner-Images),
# WINDOWS_NOTE (Signaturstatus Windows), QAS_TAG (nur release: getesteter
# QAS-Stand).
set -euo pipefail

kind="${1:?Aufruf: $0 <qas|release> <katalog.json>}"
catalog="${2:?Aufruf: $0 <qas|release> <katalog.json>}"
: "${VERSION:?}" "${REGISTRY:?}" "${GH_REPO:?}" "${IMAGE_TAG:?}" "${WINDOWS_NOTE:?}"

if [ "$kind" = "qas" ]; then
  cat <<NOTES
**QAS – nicht für Produktion.** Testfassung von Code-Prüfer ${VERSION} zur Abnahme
(Pre-Release). Nach der Abnahme entsteht daraus das Release v${VERSION}.

NOTES
else
  cat <<NOTES
Code-Prüfer ${VERSION} (Entwurf)

Abgenommen als QAS-Stand \`${QAS_TAG:-?}\`; Windows-Programmdateien und macOS-DMGs
stammen aus genau diesem Build (Windows nur neu signiert, siehe docs/RELEASE_QAS.md).

NOTES
fi
cat <<NOTES
- Windows x64: \`*_x64-setup.exe\` (${WINDOWS_NOTE})
- macOS Apple Silicon: \`*_aarch64.dmg\`, macOS Intel: \`*_x64.dmg\`
- Prüfsummen: \`SHA256SUMS.txt\` (\`sha256sum -c SHA256SUMS.txt\`)
NOTES
if [ "$kind" = "release" ]; then
  echo "- Herkunft: \`gh attestation verify <Datei> --repo ${GH_REPO}\`"
fi
cat <<NOTES

Die Runner-Images (Python, Java) sind in den Installern enthalten und werden
beim ersten Start automatisch in Docker bzw. Podman geladen. Die KI-Laufzeit
(llama.cpp) ist enthalten, das KI-Modell nicht: es wird in der App einmalig
von huggingface.co geladen oder als .gguf-Datei importiert
(Lizenzen: THIRD_PARTY_NOTICES.txt). Online-Update:
\`${REGISTRY}/code-pruefer-python:${IMAGE_TAG}\`
und \`${REGISTRY}/code-pruefer-java:${IMAGE_TAG}\`.

KI-Modelle zum Download in der App (Prüfsumme wird nach dem Download geprüft):

| Modell | Repository | Revision | Datei | Größe (Bytes) | SHA-256 |
|---|---|---|---|---|---|
NOTES
jq -r '.models[] as $m | $m.files[]
  | "| \($m.id) | \($m.repo) | `\($m.revision)` | \(.name) | \(.size) | `\(.sha256)` |"' \
  "$catalog"
