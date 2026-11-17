// Build script.
//
// Everything here is gated on the `sparkle` feature. With the feature off (the default, used by
// `cargo test`, lumen-probe, and any non-bundle build) this script does nothing, so the Sparkle
// framework does not need to be vendored to build or test the app.
//
// With `--features sparkle` it compiles the Objective-C shim (native/lumen_sparkle.m) with ARC,
// links against the vendored Sparkle.framework, and sets up the rpaths Sparkle needs: the runtime
// `@loader_path/../Frameworks` for the bundled app, plus the vendor directory so a feature build
// can run from the target directory before bundling.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=native/lumen_sparkle.m");
    println!("cargo:rerun-if-changed=native/lumen_sparkle.h");

    if std::env::var_os("CARGO_FEATURE_SPARKLE").is_none() {
        // Feature off: nothing to compile or link. The Rust updater module compiles to no-ops.
        return;
    }

    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo");
    let vendor = Path::new(&manifest_dir).join("vendor/Sparkle");
    let framework = vendor.join("Sparkle.framework");

    if !framework.exists() {
        panic!(
            "the `sparkle` feature requires the vendored Sparkle framework at {}\n\
             run scripts/dev/fetch-sparkle.sh first",
            framework.display()
        );
    }

    let headers = framework.join("Headers");

    // Compile the shim with ARC. The framework Headers dir lets `#import <Sparkle/Sparkle.h>`
    // resolve without a full framework search path at compile time.
    cc::Build::new()
        .file("native/lumen_sparkle.m")
        .flag("-fobjc-arc")
        .flag("-fmodules")
        .include("native")
        .flag(&format!("-F{}", vendor.display()))
        .flag(&format!("-I{}", headers.display()))
        .compile("lumen_sparkle");

    // Link the vendored framework.
    println!("cargo:rustc-link-lib=framework=Sparkle");
    println!("cargo:rustc-link-search=framework={}", vendor.display());

    // Runtime rpath for the bundled app: Sparkle lives in Contents/Frameworks next to the binary
    // in Contents/MacOS, i.e. @loader_path/../Frameworks.
    println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../Frameworks");

    // Dev rpath: lets a `cargo build --features sparkle` binary run straight from target/ (before
    // bundling) by resolving Sparkle from the vendor directory.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", vendor.display());
}
