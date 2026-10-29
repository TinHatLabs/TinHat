//! Regression check for the SIGABRT-on-exit blocker.
//!
//! matrix-sdk-sqlite's deadpool `SyncWrapper::drop` calls
//! `spawn_blocking_background(Runtime::Tokio1, …).unwrap()` (deadpool-sync-0.2.0
//! src/lib.rs:170). `spawn_blocking` needs `Handle::current()` in thread-local
//! storage; without it the call errors and the `.unwrap()` panics *inside a
//! destructor* → the process aborts (SIGABRT). That's why the GUI app crashed
//! when its `Client` dropped on the Qt main thread at exit.
//!
//! Run:
//!   GUARD=1 cargo run -p tinhat-dropcheck    # holds rt.enter() -> drops cleanly
//!           cargo run -p tinhat-dropcheck    # no guard -> aborts (exit 134)
//!
//! The real fix is `let _rt_guard = bridge::runtime().enter();` in the app's main().

use matrix_sdk::Client;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

fn rt() -> &'static Runtime {
    static R: OnceLock<Runtime> = OnceLock::new();
    R.get_or_init(|| Runtime::new().expect("runtime"))
}

fn main() {
    // the app's main() does the same before any client exists (matrix-sdk 0.19: no provider of its own)
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let guard_enabled = std::env::var_os("GUARD").is_some();
    // The fix under test: a runtime EnterGuard on this (non-runtime) thread.
    let _guard = guard_enabled.then(|| rt().enter());

    let store = std::env::temp_dir().join("tinhat-dropcheck-store");
    let _ = std::fs::remove_dir_all(&store);

    let client: Client = rt().block_on(async {
        Client::builder()
            .homeserver_url("http://127.0.0.1:1")
            .sqlite_store(&store, None)
            .build()
            .await
            .expect("build client")
    });

    println!("built sqlite-store client; dropping on main thread (guard={guard_enabled}) …");
    drop(client); // aborts here without the guard
    println!("OK: dropped cleanly, no abort (guard={guard_enabled})");

    let _ = std::fs::remove_dir_all(&store);
}
