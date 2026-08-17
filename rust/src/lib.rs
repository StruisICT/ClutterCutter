pub mod analysis;
pub mod datetime;
pub mod format;
pub mod types;
pub mod walk;

// Windows-only: the native Win32 GUI, the FindFirstFileExW fast walker, the NTFS
// MFT scanner, and the Windows-specific temp/cache discovery. Everything else
// compiles on Linux/macOS via the portable `walk` + `datetime` modules.
#[cfg(windows)]
pub mod gui;
#[cfg(windows)]
pub mod mft;
#[cfg(windows)]
pub mod scanner;
#[cfg(windows)]
pub mod temp;
