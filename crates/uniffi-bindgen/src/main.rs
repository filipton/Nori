// Writes the Kotlin bindings from the crates' sources: `bindings src:nori-android <out dir>`.
fn main() {
    uniffi_bindgen_kotlin_jni::main().unwrap()
}
