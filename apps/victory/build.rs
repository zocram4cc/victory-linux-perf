//! Exports the two DirectX 12 Agility SDK symbols from `victory.exe`
//! (see `src/agility.rs`). An executable exports nothing by default, and
//! D3D12 only looks for them in the process's executable.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/EXPORT:D3D12SDKVersion,DATA");
        println!("cargo:rustc-link-arg-bins=/EXPORT:D3D12SDKPath,DATA");
    }
}
