//! Exposes `crates/zed/BREWERY_VERSION` to the crate as `env!("BREWERY_VERSION")`.

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../zed/BREWERY_VERSION");
    println!("cargo:rerun-if-changed={}", path.display());
    let version = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    println!("cargo:rustc-env=BREWERY_VERSION={}", version.trim());
}
