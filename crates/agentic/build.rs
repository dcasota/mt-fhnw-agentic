//! Emit TARGET_TRIPLE at compile time so `agentic doctor` can report it.

fn main() {
    println!(
        "cargo:rustc-env=TARGET_TRIPLE={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    println!("cargo:rerun-if-changed=build.rs");
}
