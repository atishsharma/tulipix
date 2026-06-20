fn main() {
    // Compile the Slint UI tree (43 .slint files rooted at ui/main.slint) into
    // this crate. Moved out of tulipix-app so the generated code is its own
    // compilation unit. Path is relative to this crate dir (crates/tulipix-ui).
    slint_build::compile("../../ui/main.slint").expect("Slint compile failed");
}
