// The PostHog key and host are compiled in (`config::build_key`): a build after either changes
// must see the new value, not reuse the last one.
fn main() {
    println!("cargo:rerun-if-env-changed=TREK_POSTHOG_KEY");
    println!("cargo:rerun-if-env-changed=TREK_POSTHOG_HOST");
}
