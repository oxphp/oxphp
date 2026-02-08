fn main() {
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

    println!("cargo:rerun-if-changed=build.rs");
}
