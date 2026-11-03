//! The echo-cancellation bench hooks (`--features bench-aec`) never go into a release: the release trains set
//! `TINHAT_RELEASE_BUILD=1`, and that together with the feature stops the build (the same fence as the app's
//! lab-call-host / lab-update-key features).
fn main() {
    println!("cargo:rerun-if-env-changed=TINHAT_RELEASE_BUILD");
    let bench = std::env::var_os("CARGO_FEATURE_BENCH_AEC").is_some();
    let release = std::env::var("TINHAT_RELEASE_BUILD").map(|v| v.trim() == "1").unwrap_or(false);
    println!("cargo:rustc-check-cfg=cfg(tinhat_env_selftests)");
    // crypto follow-up (c): the env self-test functions (lk_publish::selftest, CallSession::selftest_audio /
    // selftest_camera) exist only outside a release (TINHAT_RELEASE_BUILD=1 builds don't contain them).
    if !release {
        println!("cargo:rustc-cfg=tinhat_env_selftests");
    }
    if bench && release {
        panic!("TINHAT_RELEASE_BUILD=1 with --features bench-aec: a release never carries the echo-cancellation bench hooks");
    }
}
