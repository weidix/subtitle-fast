#[cfg(windows)]
fn main() {
    use std::env;
    use std::path::PathBuf;

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|err| {
        panic!("CARGO_MANIFEST_DIR is required for windows resource build: {err}");
    }));
    let rc_path = manifest_dir.join("resources/windows/app.rc");
    println!("cargo:rerun-if-changed={}", rc_path.display());
    embed_resource::compile(rc_path, embed_resource::NONE);
}

#[cfg(not(windows))]
fn main() {}
