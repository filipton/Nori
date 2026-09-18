fn main() {
    // Signalsmith Stretch is C++. On Android its runtime is linked statically (`CXXSTDLIB_*` in .cargo/config.toml),
    // so the APK needs no libc++_shared.so; the static runtime leaves the C++ ABI symbols to libc++abi.a.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-lib=c++abi");
    }
}
