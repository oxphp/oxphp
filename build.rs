fn main() {
    // Loom tests are gated by `#![cfg(loom)]` and only compile when
    // invoked as `RUSTFLAGS="--cfg loom" cargo test --test loom_*`.
    // Declare the cfg to rustc so `cargo clippy --all-targets`
    // doesn't emit "unexpected cfg condition name: loom" warnings.
    println!("cargo::rustc-check-cfg=cfg(loom)");

    // The C bridge library (ext/bridge) and the PHP extension
    // (ext/oxphp_sapi.c) are compiled out-of-band by the Dockerfile
    // or by a local `make` invocation against the Makefile. When the
    // cargo feature `plugin-profiler` is active (default), those
    // out-of-band builds must be invoked with `-DOXPHP_WITH_PROFILER`
    // so the observer registration in ext/oxphp_sapi.c is compiled in;
    // the Dockerfile propagates this flag via the OXPHP_WITH_PROFILER
    // build-arg (default 1, matching the Cargo default).
    #[cfg(feature = "plugin-profiler")]
    println!("cargo:rustc-env=OXPHP_WITH_PROFILER=1");

    #[cfg(feature = "php")]
    {
        println!("cargo:rustc-link-lib=dylib=php");
        println!("cargo:rustc-link-lib=dylib=oxphp_bridge");
        println!("cargo:rustc-link-search=native=/usr/local/lib");

        // libphp.so transitive dependencies (from php-config --libs)
        println!("cargo:rustc-link-lib=dylib=xml2");
        println!("cargo:rustc-link-lib=dylib=sqlite3");
        println!("cargo:rustc-link-lib=dylib=curl");
        println!("cargo:rustc-link-lib=dylib=onig");
        println!("cargo:rustc-link-lib=dylib=readline");
        println!("cargo:rustc-link-lib=dylib=ncurses");
        println!("cargo:rustc-link-lib=dylib=argon2");
        println!("cargo:rustc-link-lib=dylib=ssl");
        println!("cargo:rustc-link-lib=dylib=crypto");
        println!("cargo:rustc-link-lib=dylib=z");

        detect_and_emit_php_cfg();
    }

    // Compile bundled .proto files into Rust types.
    // Currently only `proto/pprof.proto` (Google pprof.proto, used
    // by src/profiling/export/pprof.rs). Generated code lands in
    // $OUT_DIR/perftools.profiles.rs and is included via include!()
    // inside the exporter module. Requires `protoc` on PATH —
    // Alpine: `apk add --no-cache protobuf-dev`; macOS: `brew install
    // protobuf`.
    println!("cargo:rerun-if-changed=proto/pprof.proto");
    prost_build::compile_protos(&["proto/pprof.proto"], &["proto/"])
        .expect("failed to compile proto/pprof.proto — is protoc on PATH?");

    println!("cargo:rerun-if-changed=build.rs");
}

/// Detect the linked PHP version and emit `cargo:rustc-cfg=php_v8_X`.
///
/// Source order: `PHP_VERSION_ID` env (cross-compile / override), then
/// `php-config --vernum`, then a fallback to PHP 8.4 with a `cargo:warning`.
/// An unknown vernum panics with instructions to extend `KNOWN_PHP_VERSIONS`.
#[cfg(feature = "php")]
fn detect_and_emit_php_cfg() {
    const KNOWN_PHP_VERSIONS: &[(u32, u32, &str)] = &[
        // (min_vernum_inclusive, max_vernum_exclusive, mod_name)
        (80400, 80500, "v8_4"),
        (80500, 80600, "v8_5"),
    ];

    // Always declare check-cfg so `cargo clippy --all-targets` doesn't warn.
    for &(_, _, name) in KNOWN_PHP_VERSIONS {
        println!("cargo::rustc-check-cfg=cfg(php_{})", name);
    }

    // Force re-run when the override env changes. Deliberately NOT
    // `rerun-if-env-changed=PATH`: PATH churns constantly on dev hosts
    // (direnv / nvm / asdf hooks fire on every cd) and re-running build.rs
    // on every PATH change torches incremental builds. A developer who
    // swaps PHP toolchain runs `cargo clean` once. CI pins toolchain via
    // base image, so PATH is stable there anyway.
    println!("cargo:rerun-if-env-changed=PHP_VERSION_ID");

    let vernum = std::env::var("PHP_VERSION_ID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .or_else(|| {
            std::process::Command::new("php-config")
                .arg("--vernum")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|s| s.trim().parse::<u32>().ok())
        })
        .unwrap_or_else(|| {
            println!(
                "cargo:warning=php-config not found and PHP_VERSION_ID unset; assuming PHP 8.4 ABI"
            );
            80400
        });

    for &(lo, hi, name) in KNOWN_PHP_VERSIONS {
        if vernum >= lo && vernum < hi {
            println!("cargo:rustc-cfg=php_{}", name);
            return;
        }
    }

    panic!(
        "unsupported PHP version {vernum}. To add support:\n\
         1. append (vernum_lo, vernum_hi, \"v8_X\") to KNOWN_PHP_VERSIONS in build.rs\n\
         2. create src/php/bindings/v8_X.rs (start by copying the previous version)\n\
         3. wire #[cfg(php_v8_X)] mod v8_X; pub use v8_X::*; in src/php/bindings/mod.rs\n\
         Known versions: {KNOWN_PHP_VERSIONS:?}"
    );
}
