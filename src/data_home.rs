//! tokensave's own data folder, `~/.tokensave`, relocatable with
//! `TOKENSAVE_HOME`.
//!
//! The variable works like `CARGO_HOME` and `RUSTUP_HOME`: set and not empty,
//! it names the folder itself (`global.db`, `config.toml`, `pricing.json`,
//! `servers/` and the rest live directly in it); unset or empty, the folder is
//! `.tokensave` under the home directory. A relative value is taken from the
//! current directory, as Cargo takes a relative `CARGO_HOME`.
//!
//! Each caller passes the home directory it already resolves, because the two
//! lookups in use differ on Windows (`dirs::home_dir` asks the known-folder
//! API, [`crate::agents::home_dir`] reads `HOME` then `USERPROFILE`; upstream
//! #575). Unifying them changes behaviour and is not this module's job, so an
//! unset variable leaves every path exactly where it was.
//!
//! The test suite sets the variable from outside the process, in
//! `.cargo/config.toml`, so `cargo test` never writes the developer's real
//! data (inkcast #251). Nothing here calls `std::env::set_var`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// tokensave's data folder: `TOKENSAVE_HOME` when set, else `.tokensave` under
/// `home` (the caller's own home lookup). `None` only when the variable is
/// unset and `home` is `None`.
pub fn tokensave_home(home: Option<PathBuf>) -> Option<PathBuf> {
    resolve(
        std::env::var_os("TOKENSAVE_HOME"),
        home,
        std::env::current_dir().ok(),
    )
}

/// The data folder for an explicit `home`, as an install or uninstall for
/// that home sees it.
///
/// `TOKENSAVE_HOME` relocates the data of the user's own home only: when
/// `home` is the user's home (what [`crate::agents::home_dir`] returns, which
/// is how `main` builds `InstallContext::home`), the variable applies; any
/// other `home`, such as a test's temporary one, keeps `<home>/.tokensave`, so
/// a function handed a home never reads data from somewhere else.
pub fn tokensave_home_for(home: &Path) -> PathBuf {
    let own_home = crate::agents::home_dir();
    if own_home.as_deref().is_some_and(|own| same_dir(own, home)) {
        if let Some(dir) = tokensave_home(own_home) {
            return dir;
        }
    }
    home.join(".tokensave")
}

/// [`tokensave_home`] with its inputs passed in, so it is tested without
/// touching the process environment.
fn resolve(var: Option<OsString>, home: Option<PathBuf>, cwd: Option<PathBuf>) -> Option<PathBuf> {
    match var.filter(|value| !value.is_empty()).map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Some(dir),
        Some(dir) => Some(cwd.map_or_else(|| dir.clone(), |cwd| cwd.join(&dir))),
        None => home.map(|home| home.join(".tokensave")),
    }
}

/// Whether two paths name one directory, following symlinks when both exist
/// (macOS reports `/var/…` and `/private/var/…` for the same folder).
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/user")
    }

    #[test]
    fn unset_keeps_the_folder_under_home() {
        assert_eq!(
            resolve(None, Some(home()), None),
            Some(PathBuf::from("/home/user/.tokensave"))
        );
    }

    #[test]
    fn empty_counts_as_unset() {
        assert_eq!(
            resolve(Some(OsString::new()), Some(home()), None),
            Some(PathBuf::from("/home/user/.tokensave"))
        );
    }

    #[test]
    fn an_absolute_value_names_the_folder_itself() {
        assert_eq!(
            resolve(Some("/data/ts".into()), Some(home()), Some("/work".into())),
            Some(PathBuf::from("/data/ts"))
        );
    }

    #[test]
    fn a_relative_value_is_taken_from_the_current_directory() {
        assert_eq!(
            resolve(Some("ts-data".into()), Some(home()), Some("/work".into())),
            Some(PathBuf::from("/work/ts-data"))
        );
    }

    #[test]
    fn the_variable_does_not_need_a_home() {
        assert_eq!(
            resolve(Some("/data/ts".into()), None, None),
            Some(PathBuf::from("/data/ts"))
        );
    }

    #[test]
    fn no_variable_and_no_home_is_none() {
        assert_eq!(resolve(None, None, None), None);
    }

    #[test]
    fn another_home_keeps_its_own_folder() {
        let other = tempfile::tempdir().unwrap();
        assert_eq!(
            tokensave_home_for(other.path()),
            other.path().join(".tokensave")
        );
    }
}
