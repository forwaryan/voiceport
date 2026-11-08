use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=src/local_runtime");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => panic!("unsupported macOS architecture"),
    };
    let object = out.join("local_runtime.o");
    assert!(
        Command::new("/usr/bin/clang++")
            .args([
                "-std=c++17",
                "-mmacosx-version-min=13.0",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-arch",
                arch,
                "-c",
                "src/local_runtime/bridge.cc",
                "-o"
            ])
            .arg(&object)
            .status()
            .expect("macOS Command Line Tools clang")
            .success()
    );
    assert!(
        Command::new("/usr/bin/ar")
            .arg("rcs")
            .arg(out.join("libvoiceport_local.a"))
            .arg(object)
            .status()
            .expect("macOS Command Line Tools ar")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=voiceport_local");
    println!("cargo:rustc-link-lib=c++");
}
