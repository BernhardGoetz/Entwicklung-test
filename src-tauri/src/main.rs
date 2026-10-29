// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The same executable parses imported documents in a limited child
    // process (docs/worker.rs); that must happen before Tauri starts.
    if let Some(code) = code_pruefer_lib::docs::worker::run_if_requested() {
        std::process::exit(code);
    }
    code_pruefer_lib::run()
}
