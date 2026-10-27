//! inkcast #251: `cargo test` must never write the developer's real
//! `~/.tokensave`. `.cargo/config.toml` points `TOKENSAVE_HOME` at
//! `target/test-tokensave-home` from outside the process; this test fails
//! loudly when that value did not reach the test process (cargo invoked from
//! another directory, so the config file was not read; a runner that ignores
//! `[env]`; or the entry removed) or when an override names the real data
//! folder, instead of letting the run write there unnoticed.

use std::path::{Path, PathBuf};

const HOW_TO_FIX: &str = "run cargo from the repository root so .cargo/config.toml \
     applies, or set TOKENSAVE_HOME to a scratch folder";

/// `path` made absolute against the current directory, as
/// `tokensave::data_home` does, with its longest existing ancestor resolved
/// through symlinks, so an alias of a folder compares equal to the folder
/// whether or not the rest of the path exists yet. Nothing is created.
fn location(path: &Path) -> PathBuf {
    let path = std::env::current_dir()
        .expect("the current directory")
        .join(path);
    path.ancestors()
        .find_map(|ancestor| {
            let mut real = ancestor.canonicalize().ok()?;
            real.extend(path.strip_prefix(ancestor).ok()?);
            Some(real)
        })
        .unwrap_or(path)
}

#[test]
fn tests_use_a_sandboxed_data_folder() {
    let configured = std::env::var_os("TOKENSAVE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("TOKENSAVE_HOME is not set, so this run writes to the real ~/.tokensave: {HOW_TO_FIX}")
        });
    let sandbox = std::env::current_dir()
        .expect("the current directory")
        .join(configured);

    let global_db = tokensave::global_db::global_db_path().expect("a global.db path");
    assert_eq!(global_db, sandbox.join("global.db"));
    let benchmark = tokensave::user_config::benchmark_path().expect("a benchmark.toml path");
    assert_eq!(benchmark, sandbox.join("benchmark.toml"));

    // Both home lookups in use; they differ on Windows (upstream #575).
    let sandbox = location(&sandbox);
    for home in [tokensave::agents::home_dir(), dirs::home_dir()]
        .into_iter()
        .flatten()
    {
        let real = location(&home.join(".tokensave"));
        assert!(
            !sandbox.starts_with(&real),
            "TOKENSAVE_HOME resolves to {}, inside the real {}: {HOW_TO_FIX}",
            sandbox.display(),
            real.display()
        );
    }
}
