//! Writes the manifest of one platform package: `plugin.json` narrowed to
//! that platform's executable. `plugin.json` itself is written by hand.
//!
//!     cargo run --bin manifest -- -platform linux-amd64 -out dist/stage/linux-amd64/plugin.json

use std::path::{Path, PathBuf};

use plugin_log_analytics_tantivy::manifest;

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
    if platform.is_empty() || output.is_empty() {
        return Err("-platform and -out are required, plugin.json is written by hand".into());
    }

    let source = if input.is_empty() {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugin.json")
    } else {
        PathBuf::from(&input)
    };
    let source = std::fs::read_to_string(&source).map_err(|e| format!("{}: {e}", source.display()))?;
    let text = manifest::narrow(&source, &platform)?;
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
