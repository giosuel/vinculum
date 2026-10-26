include!("../version.rs");

fn main() {
    set_version();
    // Debug info names the elements, so tests can find them (ElementHandle); it's only a few
    // strings, and the AUR package runs the tests in a release build.
    let config = slint_build::CompilerConfiguration::new().with_debug_info(true);
    slint_build::compile_with_config("ui/main.slint", config).expect("the Slint UI compiles");
}
