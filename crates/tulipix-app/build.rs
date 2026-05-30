fn main() {
    slint_build::compile("../../ui/main.slint").expect("Slint compile failed");
    // Embedded player links libmpv directly (render API). libmpv.so ships with
    // the system mpv package on Linux; packaging bundles it elsewhere.
    println!("cargo:rustc-link-lib=mpv");
}
