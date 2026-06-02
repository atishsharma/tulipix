fn main() {
    slint_build::compile("../../ui/main.slint").expect("Slint compile failed");
    // Trends feed list is include_str!'d from podc.md — rebuild when it changes.
    println!("cargo:rerun-if-changed=../../podc.md");
    // Embedded player links libmpv directly (render API). libmpv.so ships with
    // the system mpv package on Linux; packaging bundles it elsewhere.
    println!("cargo:rustc-link-lib=mpv");
}
