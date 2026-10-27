//! The dashboard page's files, embedded at compile time from
//! `dashboard/dist/` by `build.rs`: `(path, bytes)` pairs, the path relative
//! to `dist/` with `/` separators. Empty when the page was not built, and the
//! server then answers `/` with 503.

include!(concat!(env!("OUT_DIR"), "/dashboard_assets.rs"));
