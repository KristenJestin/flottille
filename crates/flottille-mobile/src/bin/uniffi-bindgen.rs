//! The generator of the Kotlin bindings, from the built library (`uniffi-bindgen generate
//! --language kotlin --out-dir <dir> <libflottille_mobile.so>`): the Gradle build runs it.

fn main() {
    uniffi::uniffi_bindgen_main()
}
