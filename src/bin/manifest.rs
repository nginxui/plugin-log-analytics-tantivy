//! Writes `plugin.json`, or the manifest of one platform package.
//!
//!     cargo run --bin manifest
//!     cargo run --bin manifest -- -platform linux-amd64 -out dist/stage/linux-amd64/plugin.json

use std::path::{Path, PathBuf};

use plugin_log_analytics_tantivy::manifest;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run() -> Result<(), String> {
    let mut platform = String::new();
    let mut input = String::new();
    let mut output = String::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let slot = match arg.trim_start_matches('-') {
            "platform" => &mut platform,
            "in" => &mut input,
            "out" => &mut output,
            other => return Err(format!("unknown flag {other}")),
        };
        *slot = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
    }

    let root = root();
    let text = if platform.is_empty() {
        let fragment = std::fs::read_to_string(root.join("webapp/dist/manifest.webapp.json")).unwrap_or_default();
        if output.is_empty() {
            output = root.join("plugin.json").to_string_lossy().into_owned();
        }
        manifest::render(&manifest::build(manifest::shared_from_fragment(&fragment)))
    } else {
        if output.is_empty() {
            return Err("-platform needs -out, the committed plugin.json keeps every platform".into());
        }
        let source = if input.is_empty() { root.join("plugin.json") } else { PathBuf::from(&input) };
        let source = std::fs::read_to_string(&source).map_err(|e| format!("{}: {e}", source.display()))?;
        manifest::narrow(&source, &platform)?
    };
    std::fs::write(Path::new(&output), &text).map_err(|e| format!("{output}: {e}"))?;
    println!("wrote {output} ({} bytes)", text.len());
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("manifest: {e}");
        std::process::exit(1);
    }
}
