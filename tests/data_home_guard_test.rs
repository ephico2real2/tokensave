//! inkcast #251: `cargo test` must never write the developer's real
//! `~/.tokensave`. `.cargo/config.toml` points `TOKENSAVE_HOME` at
//! `target/test-tokensave-home` from outside the process; this test fails
//! loudly when that value did not reach the test process (cargo invoked from
//! another directory, so the config file was not read; a runner that ignores
//! `[env]`; or the entry removed), instead of letting the run write to the
//! real data folder unnoticed.

use std::path::PathBuf;

const HOW_TO_FIX: &str = "run cargo from the repository root so .cargo/config.toml \
     applies, or set TOKENSAVE_HOME to a scratch folder";

#[test]
fn tests_use_a_sandboxed_data_folder() {
    let sandbox = std::env::var_os("TOKENSAVE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("TOKENSAVE_HOME is not set, so this run writes to the real ~/.tokensave: {HOW_TO_FIX}")
        });

    let global_db = tokensave::global_db::global_db_path().expect("a global.db path");
    assert_eq!(global_db, sandbox.join("global.db"));
    let benchmark = tokensave::user_config::benchmark_path().expect("a benchmark.toml path");
    assert_eq!(benchmark, sandbox.join("benchmark.toml"));

    if let Some(home) = tokensave::agents::home_dir() {
        let real = home.join(".tokensave");
        assert!(
            !global_db.starts_with(&real),
            "TOKENSAVE_HOME points into the real {}: {HOW_TO_FIX}",
            real.display()
        );
    }
}
