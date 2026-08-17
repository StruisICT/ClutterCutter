// GUI entry point. The actual window/message-loop machinery lives in
// `cluttercutter::gui` so it can be exercised from tests / other binaries.
//
// This binary is the native Win32 GUI and is Windows-only. On other platforms
// it compiles to a stub that points at the cross-platform frontends, so the
// whole crate still builds on Linux/macOS for CI and the egui port.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    cluttercutter::gui::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "The native Win32 GUI (`cluttercutter`) is Windows-only.\n\
         On Linux/macOS use the cross-platform build (`cluttercutter-gui`) or the CLI (`cluttercutter-cli`)."
    );
    std::process::exit(1);
}
