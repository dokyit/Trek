include!("../../script/static_vcruntime.rs");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    static_vcruntime();
}
