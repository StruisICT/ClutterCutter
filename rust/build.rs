fn main() {
    // Compiles app.rc which embeds the ClutterCutter.ico icon and basic
    // version info into the GUI binary. Linker is invoked transparently.
    // embed-resource 3.x returns a must-use result; the manifest is optional
    // (app.rc carries only an icon + version info, no side-by-side manifest).
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embed app.rc resources");
}
