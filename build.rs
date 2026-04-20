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
