use std::env;

/// Default server baked into the binary: release builds point at production,
/// debug builds at localhost. `GOMOKU_DEFAULT_SERVER` overrides both
fn main() {
    println!("cargo:rerun-if-env-changed=GOMOKU_DEFAULT_SERVER");
    println!("cargo:rerun-if-env-changed=PROFILE");
    let url = env::var("GOMOKU_DEFAULT_SERVER").unwrap_or_else(|_| {
        if env::var("PROFILE").as_deref() == Ok("release") {
            "https://api-gomoku.seol.pro".to_owned()
        } else {
            "http://localhost:3000".to_owned()
        }
    });
    println!("cargo:rustc-env=GOMOKU_DEFAULT_SERVER={url}");
}
