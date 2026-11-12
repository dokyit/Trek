// whisper.cpp is built with the static Visual C++ runtime (vendor/whisper-rs-sys), so this crate's
// own programs (its tests, and the probe example) link the runtime as the programs Trek ships do.
include!("../../script/static_vcruntime.rs");

fn main() {
    static_vcruntime();
}
