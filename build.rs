//! Windows resources: the OS application icon. GPUI's Windows platform loads
//! icon resource 1 for the window and taskbar, and Windows uses the same
//! resource for the executable itself.

fn main() {
    println!("cargo:rerun-if-changed=assets/spur.ico");
    println!("cargo:rerun-if-changed=assets/spur.rc");
    embed_resource::compile("assets/spur.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
