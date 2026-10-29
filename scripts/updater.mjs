#!/usr/bin/env node
// Hilfsskript für den App-Updater (docs/UPDATER.md). Ohne Abhängigkeiten
// (nur node:crypto), läuft im Release-Workflow und lokal.
//
//   node scripts/updater.mjs config [<ausgabe.json>]
//       Schreibt das Konfigurations-Overlay (Standard:
//       src-tauri/tauri.updater.conf.json) mit dem öffentlichen Schlüssel aus
//       der Umgebungsvariable TAURI_UPDATER_PUBKEY. Ohne Schlüssel: leeres
//       Overlay "{}" und eine Warnung – die App läuft dann ohne Updater.
//
//   node scripts/updater.mjs verify <datei> [<datei.sig>] [--version <v>]
//       Prüft die Updater-Signatur (minisign, wie tauri-plugin-updater) mit
//       TAURI_UPDATER_PUBKEY; mit --version muss die Signatur genau diese
//       Version nennen (requireSignedVersion). Exit-Code 1 bei Fehler.
//
//   node scripts/updater.mjs manifest --version <v> --base-url <url>
//       [--notes-file <datei>] [--sig-dir <ordner>] --out <latest.json>
//       <plattform>=<datei> ...
//       Schreibt latest.json für das GitHub-Release. Plattformen:
//       windows-x86_64, darwin-aarch64, darwin-x86_64. Die Signatur steht in
//       <datei>.sig (bzw. <ordner>/<dateiname>.sig) und wird vorher geprüft
//       (mit --version).
import { createHash, createPublicKey, verify as edVerify } from "node:crypto";
import { createReadStream, readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";

const PLATFORMS = ["windows-x86_64", "darwin-aarch64", "darwin-x86_64"];
// Asset-Namen, die GitHub unverändert übernimmt (sonst passt die URL nicht).
const SAFE_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
// DER-Präfix eines Ed25519-SubjectPublicKeyInfo (RFC 8410).
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

function fail(message) {
  console.error(`::error::${message}`);
  process.exit(1);
}

function b64(text, what) {
  const s = text.trim();
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(s)) throw new Error(`${what}: kein Base64`);
  return Buffer.from(s, "base64");
}

/** Öffentlicher Schlüssel im Tauri-Format: Base64 der minisign-.pub-Datei. */
function parsePublicKey(tauriPubkey) {
  const lines = b64(tauriPubkey, "Öffentlicher Schlüssel").toString("utf8").split(/\r?\n/);
  if (!lines[0]?.startsWith("untrusted comment:") || !lines[1])
    throw new Error("Öffentlicher Schlüssel: keine minisign-Schlüsseldatei");
  const bin = b64(lines[1], "Öffentlicher Schlüssel");
  if (bin.length !== 42 || bin.toString("latin1", 0, 2) !== "Ed")
    throw new Error("Öffentlicher Schlüssel: falsches Format (erwartet Ed25519, 42 Bytes)");
  return {
    keyId: bin.subarray(2, 10),
    key: createPublicKey({ key: Buffer.concat([ED25519_SPKI_PREFIX, bin.subarray(10, 42)]), format: "der", type: "spki" }),
  };
}

/** Signatur im Tauri-Format: Base64 der minisign-.minisig-Datei (= Inhalt der .sig). */
function parseSignature(tauriSignature) {
  const lines = b64(tauriSignature, "Signatur").toString("utf8").split(/\r?\n/);
  if (lines.length < 4 || !lines[2].startsWith("trusted comment: "))
    throw new Error("Signatur: keine minisign-Signatur");
  const bin = b64(lines[1], "Signatur");
  const global = b64(lines[3], "Signatur");
  if (bin.length !== 74 || global.length !== 64) throw new Error("Signatur: falsche Länge");
  const alg = bin.toString("latin1", 0, 2);
  if (alg !== "ED" && alg !== "Ed") throw new Error(`Signatur: unbekannter Algorithmus ${alg}`);
  return {
    prehashed: alg === "ED",
    keyId: bin.subarray(2, 10),
    signature: bin.subarray(10, 74),
    trustedComment: lines[2].slice("trusted comment: ".length),
    global,
  };
}

async function blake2b512(path) {
  const h = createHash("blake2b512");
  for await (const chunk of createReadStream(path)) h.update(chunk);
  return h.digest();
}

/**
 * Prüft wie minisign-verify (tauri-plugin-updater): Schlüssel-ID, Signatur
 * über BLAKE2b-512 der Datei (bzw. die Datei bei alten Signaturen), globale
 * Signatur über Signatur + vertrauenswürdigen Kommentar, optional die
 * signierte Version ("version:<v>" im Kommentar). Wirft bei Fehlern.
 */
async function verifyFile(path, tauriSignature, tauriPubkey, version) {
  const pub = parsePublicKey(tauriPubkey);
  const sig = parseSignature(tauriSignature);
  if (!pub.keyId.equals(sig.keyId))
    throw new Error(
      `${basename(path)}: mit einem anderen Schlüssel signiert (Schlüssel-ID ${Buffer.from(sig.keyId).reverse().toString("hex").toUpperCase()}, erwartet ${Buffer.from(pub.keyId).reverse().toString("hex").toUpperCase()})`,
    );
  const message = sig.prehashed ? await blake2b512(path) : readFileSync(path);
  if (!edVerify(null, message, pub.key, sig.signature)) throw new Error(`${basename(path)}: Signatur ungültig`);
  const global = Buffer.concat([sig.signature, Buffer.from(sig.trustedComment, "utf8")]);
  if (!edVerify(null, global, pub.key, sig.global)) throw new Error(`${basename(path)}: globale Signatur ungültig`);
  if (version !== undefined) {
    const signed = sig.trustedComment
      .split("\t")
      .find((f) => f.startsWith("version:"))
      ?.slice("version:".length);
    if (signed !== version)
      throw new Error(`${basename(path)}: signiert für Version ${signed ?? "(keine)"}, erwartet ${version}`);
  }
  return sig.trustedComment;
}

function pubkeyFromEnv() {
  const k = (process.env.TAURI_UPDATER_PUBKEY ?? "").trim();
  if (k === "") fail("TAURI_UPDATER_PUBKEY ist nicht gesetzt.");
  return k;
}

function cmdConfig([out = "src-tauri/tauri.updater.conf.json"]) {
  const pubkey = (process.env.TAURI_UPDATER_PUBKEY ?? "").trim();
  if (pubkey === "") {
    writeFileSync(out, "{}\n");
    console.log(
      "::warning::Variable TAURI_UPDATER_PUBKEY ist nicht gesetzt – diese Version enthält keinen Updater (docs/UPDATER.md).",
    );
    return;
  }
  let pub;
  try {
    pub = parsePublicKey(pubkey);
  } catch (e) {
    fail(`TAURI_UPDATER_PUBKEY: ${e.message}`);
  }
  const overlay = {
    plugins: {
      updater: {
        pubkey,
        // Die Signatur muss die angekündigte Version nennen (schützt vor
        // einer manipulierten latest.json mit alter, gültig signierter Datei).
        requireSignedVersion: true,
        // NSIS-Installer ohne Rückfragen, nur mit Fortschrittsanzeige
        // (/P /UPDATE /R); startet die App danach neu.
        windows: { installMode: "passive" },
      },
    },
  };
  writeFileSync(out, `${JSON.stringify(overlay, null, 2)}\n`);
  // minisign zeigt die Schlüssel-ID in umgekehrter Byte-Reihenfolge.
  const keyId = Buffer.from(pub.keyId).reverse().toString("hex").toUpperCase();
  console.log(`Updater-Overlay ${out} geschrieben (Schlüssel-ID ${keyId}).`);
}

async function cmdVerify(args) {
  const i = args.indexOf("--version");
  const version = i >= 0 ? args[i + 1] : undefined;
  const files = i >= 0 ? args.filter((_, j) => j !== i && j !== i + 1) : args;
  const [file, sigFile = `${file}.sig`] = files;
  if (!file) fail("Aufruf: updater.mjs verify <datei> [<datei.sig>] [--version <v>]");
  try {
    const comment = await verifyFile(file, readFileSync(sigFile, "utf8"), pubkeyFromEnv(), version);
    console.log(`${basename(file)}: Signatur gültig (${comment})`);
  } catch (e) {
    fail(e.message);
  }
}

async function cmdManifest(args) {
  const opts = {};
  const entries = [];
  for (let i = 0; i < args.length; i++) {
    if (args[i].startsWith("--")) opts[args[i].slice(2)] = args[++i];
    else entries.push(args[i]);
  }
  const { version, out } = opts;
  const baseUrl = (opts["base-url"] ?? "").replace(/\/+$/, "");
  if (!version || !out || !baseUrl.startsWith("https://") || entries.length === 0)
    fail("Aufruf: updater.mjs manifest --version <v> --base-url https://… [--notes-file <f>] [--sig-dir <d>] --out <latest.json> <plattform>=<datei> …");
  const pubkey = pubkeyFromEnv();
  const platforms = {};
  for (const entry of entries) {
    const eq = entry.indexOf("=");
    const platform = entry.slice(0, eq);
    const file = entry.slice(eq + 1);
    if (!PLATFORMS.includes(platform)) fail(`Unbekannte Plattform ${platform} (erlaubt: ${PLATFORMS.join(", ")})`);
    if (platforms[platform]) fail(`Plattform ${platform} doppelt`);
    const name = basename(file);
    if (!SAFE_NAME.test(name)) fail(`Dateiname ${name} enthält Zeichen, die GitHub im Asset-Namen ersetzt.`);
    const sigPath = opts["sig-dir"] ? `${opts["sig-dir"]}/${name}.sig` : `${file}.sig`;
    const signature = readFileSync(sigPath, "utf8").trim();
    try {
      await verifyFile(file, signature, pubkey, version);
    } catch (e) {
      fail(e.message);
    }
    platforms[platform] = { signature, url: `${baseUrl}/${encodeURIComponent(name)}` };
  }
  const notes = opts["notes-file"] ? readFileSync(opts["notes-file"], "utf8").replace(/^﻿/, "").trim() : "";
  const manifest = {
    version,
    notes,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    platforms,
  };
  writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`${out}: Version ${version}, Plattformen ${Object.keys(platforms).join(", ")}`);
}

const [cmd, ...rest] = process.argv.slice(2);
if (cmd === "config") cmdConfig(rest);
else if (cmd === "verify") await cmdVerify(rest);
else if (cmd === "manifest") await cmdManifest(rest);
else fail("Aufruf: updater.mjs <config|verify|manifest> …");
