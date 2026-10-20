include!("../version.rs");

fn main() {
    set_version();
    slint_build::compile("ui/main.slint").expect("the Slint UI compiles");
}
