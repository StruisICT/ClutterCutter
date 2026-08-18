fn main() {
    // app.rc embeds the ClutterCutter.ico icon + version info into the Windows
    // GUI binary. It's a Win32 resource script (rc/windres), so only compile it
    // when the *target* is Windows — CARGO_CFG_WINDOWS is set by Cargo for the
    // target being built, letting the crate cross-check/build cleanly on Linux.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embed app.rc resources");
    }
}
