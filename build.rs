use std::{
    fs,
    path::{Path, PathBuf},
};

fn main() {
    let out_path = Path::new("src/resources/logo.ansi");
    let logo_bytes = include_bytes!("src/resources/logo.png");
    let ansi = logo_art::image_to_ansi(logo_bytes, 90);
    fs::write(out_path, ansi).unwrap();
    println!("cargo::rerun-if-changed=src/resources/logo.png");

    embed_dashboard();

    // Vendored WGSL grammar — compiled only when lang-wgsl is enabled.
    // Using vendored sources avoids pulling in tree-sitter-wgsl 0.0.6 which was
    // built against the incompatible tree-sitter 0.20 API.
    if std::env::var("CARGO_FEATURE_LANG_WGSL").is_ok() {
        let wgsl_dir = Path::new("vendor/tree-sitter-wgsl/src");
        cc::Build::new()
            .include(wgsl_dir)
            .file(wgsl_dir.join("parser.c"))
            .file(wgsl_dir.join("scanner.c"))
            .warnings(false)
            .compile("tree_sitter_wgsl");
        println!("cargo::rerun-if-changed=vendor/tree-sitter-wgsl/src/parser.c");
        println!("cargo::rerun-if-changed=vendor/tree-sitter-wgsl/src/scanner.c");
    }

    // Vendored ActionScript grammar (tree-sitter-actionscript, jcs090218).
    // parser.c only — the grammar has no external scanner.
    if std::env::var("CARGO_FEATURE_LANG_ACTIONSCRIPT").is_ok() {
        let as_dir = Path::new("vendor/tree-sitter-actionscript/src");
        cc::Build::new()
            .include(as_dir)
            .file(as_dir.join("parser.c"))
            .warnings(false)
            .compile("tree_sitter_actionscript");
        println!("cargo::rerun-if-changed=vendor/tree-sitter-actionscript/src/parser.c");
    }

    // Vendored GDScript grammar (tree-sitter-gdscript, PrestonKnopp).
    // parser.c + scanner.c — the grammar has an external scanner.
    if std::env::var("CARGO_FEATURE_LANG_GDSCRIPT").is_ok() {
        let gdscript_dir = Path::new("vendor/tree-sitter-gdscript/src");
        cc::Build::new()
            .include(gdscript_dir)
            .file(gdscript_dir.join("parser.c"))
            .file(gdscript_dir.join("scanner.c"))
            .warnings(false)
            .compile("tree_sitter_gdscript");
        println!("cargo::rerun-if-changed=vendor/tree-sitter-gdscript/src/parser.c");
        println!("cargo::rerun-if-changed=vendor/tree-sitter-gdscript/src/scanner.c");
    }
}

/// Writes `$OUT_DIR/dashboard_assets.rs`: every file under `dashboard/dist/`
/// (the page, built by Vite) as `(path, include_bytes!(...))`, so the binary
/// serves the page without Node or the files at run time.
///
/// Cargo reruns a build script every build when a watched path does not
/// exist, which would recompile the C grammars above each time, so only an
/// existing folder is watched: `dashboard/dist`, else `dashboard`, where
/// `dist` appears. A build made before `dashboard/` existed does not notice
/// it appear until something else reruns this script.
fn embed_dashboard() {
    let dist = Path::new("dashboard/dist");
    let mut files = Vec::new();
    if dist.is_dir() {
        println!("cargo::rerun-if-changed=dashboard/dist");
        collect_files(dist, &mut files);
    } else if Path::new("dashboard").is_dir() {
        println!("cargo::rerun-if-changed=dashboard");
    }
    files.sort();

    let mut code = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for file in &files {
        let relative: Vec<String> = file
            .strip_prefix(dist)
            .unwrap()
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        let absolute = fs::canonicalize(file).unwrap();
        code.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            relative.join("/"),
            absolute
        ));
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("dashboard_assets.rs");
    fs::write(out, code).unwrap();
}

/// Every file below `dir`, hidden ones (`.DS_Store`) left out.
fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            files.push(path);
        }
    }
}
