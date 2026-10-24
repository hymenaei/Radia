fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS") {
        Ok(target_os) if target_os == "linux" => println!("cargo:rustc-link-lib=dylib=stdc++"),
        Ok(target_os) if target_os == "macos" => println!("cargo:rustc-link-lib=dylib=c++"),
        Ok(_) => {}
        Err(error) => panic!("CARGO_CFG_TARGET_OS is required: {error}"),
    }
}
