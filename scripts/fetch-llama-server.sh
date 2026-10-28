#!/usr/bin/env bash
# Holt llama-server aus einem offiziellen llama.cpp-Release (Version und
# SHA-256 je Plattform fest gepinnt), prüft die Prüfsumme und legt
# llama-server samt der benötigten Bibliotheken in den Ressourcenordner, den
# Release-Builds mitliefern (src-tauri/tauri.release.conf.json -> llama/).
#
#   scripts/fetch-llama-server.sh <win-x64|macos-arm64|macos-x64|linux-x64> [Zielordner]
#
# linux-x64 ist nur für die Entwicklung gedacht (Debug-Build mit
# CODE_PRUEFER_LLAMA_SERVER=<Zielordner>/llama-server).
# Der Zielordner (Standard: src-tauri/resources/llama) wird geleert.
set -euo pipefail

# Muss zu LLAMA_CPP_VERSION in src-tauri/src/ai/llama.rs passen (Unit-Test).
version="b11700"
base="https://github.com/ggml-org/llama.cpp/releases/download/${version}"

platform="${1:-}"
case "$platform" in
  win-x64)
    asset="llama-${version}-bin-win-cpu-x64.zip"
    sha256="4e2af6b2b39691cd84267a02839596ed6653ea2dde5ee9cdbfeb4dd9677eecbb"
    ;;
  macos-arm64)
    asset="llama-${version}-bin-macos-arm64.tar.gz"
    sha256="75d795f8826e09893d44e040742ca5dd604d9402dc40393aed0aea6866aadd73"
    ;;
  macos-x64)
    asset="llama-${version}-bin-macos-x64.tar.gz"
    sha256="957911e205758c5ffc5e835e631dd428ba7a46c91f8824e3a3e1a2f107aa7c79"
    ;;
  linux-x64)
    asset="llama-${version}-bin-ubuntu-x64.tar.gz"
    sha256="51842656431fc20928123e0c88598f7604a2fe2031fb4e7d45943316817de024"
    ;;
  *)
    echo "Aufruf: $0 <win-x64|macos-arm64|macos-x64|linux-x64> [Zielordner]" >&2
    exit 2
    ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${2:-$root/src-tauri/resources/llama}"

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "Lade $asset …"
curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$work/$asset" "$base/$asset"
actual="$(sha256_of "$work/$asset")"
if [ "$actual" != "$sha256" ]; then
  echo "SHA-256 stimmt nicht: erwartet $sha256, erhalten $actual" >&2
  exit 1
fi

mkdir -p "$work/x"
case "$asset" in
  *.zip) unzip -q "$work/$asset" -d "$work/x" ;;
  *) tar -xzf "$work/$asset" -C "$work/x" ;;
esac
# Die tar-Archive enthalten einen Ordner llama-<version>/, das zip nicht.
src="$work/x"
if [ -d "$work/x/llama-${version}" ]; then
  src="$work/x/llama-${version}"
fi

rm -rf "$out"
mkdir -p "$out"
case "$platform" in
  win-x64)
    # llama-server.exe lädt llama-server-impl.dll; ggml.dll wählt zur
    # Laufzeit die passende ggml-cpu-<CPU>.dll (AVX2, AVX-512, …).
    for f in llama-server.exe llama-server-impl.dll llama-common.dll llama.dll mtmd.dll \
      ggml.dll ggml-base.dll libomp.dll LICENSE-LLVM-OpenMP; do
      cp "$src/$f" "$out/"
    done
    cp "$src"/ggml-cpu-*.dll "$out/"
    ;;
  macos-*)
    # Die Bibliotheken werden als lib<name>.0.dylib referenziert
    # (@rpath = @loader_path), im Archiv Symlinks auf lib<name>.0.<x.y>.dylib;
    # nur diese Namen werden (aufgelöst) übernommen.
    cp "$src/llama-server" "$src/libllama-server-impl.dylib" "$out/"
    for f in "$src"/lib*.0.dylib; do
      if [ -L "$f" ]; then
        cp -L "$f" "$out/"
      fi
    done
    ;;
  linux-x64)
    cp "$src/llama-server" "$out/"
    cp -a "$src"/lib*.so* "$out/"
    ;;
esac
if [ -f "$src/LICENSE" ]; then
  cp "$src/LICENSE" "$out/LICENSE-llama.cpp"
fi
printf '%s %s %s\n' "$version" "$asset" "$sha256" >"$out/VERSION.txt"

echo "llama-server ${version} (${platform}) -> $out"
ls -l "$out"
