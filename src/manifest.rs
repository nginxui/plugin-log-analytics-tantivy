//! `plugin.json` is written by hand. The packaging narrows it to one platform
//! per package with `narrow`, through `cargo run --bin manifest`. The tests
//! below keep it in line with the code, Cargo.toml and the webapp build.

use serde_json::Value;

/// Narrows a manifest to one `<os>-<arch>` executable. A per platform package
/// declares exactly the platform it ships. Every other member keeps its order
/// and value, so the narrowing works with any member the contract adds.
pub fn narrow(text: &str, platform: &str) -> Result<String, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| format!("decode manifest: {e}"))?;
    let executables = value
        .get_mut("server")
        .and_then(|s| s.get_mut("executables"))
        .and_then(Value::as_object_mut)
        .ok_or("the manifest has no server executables")?;
    let path = executables
        .get(platform)
        .cloned()
        .ok_or_else(|| format!("the manifest declares no executable for {platform}"))?;
    executables.clear();
    executables.insert(platform.to_owned(), path);
    layout(&value)
}

/// The committed layout: two space indentation and a trailing newline.
fn layout(value: &Value) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGIN_JSON: &str = include_str!("../plugin.json");

    fn manifest() -> Value {
        serde_json::from_str(PLUGIN_JSON).expect("plugin.json decodes")
    }

    fn texts(value: &Value) -> Vec<String> {
        value.as_object().map(|o| o.values().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
    }

    /// Package relative path build.sh gives the executable of a platform.
    fn executable_path(platform: &str) -> String {
        let ext = if platform.starts_with("windows-") { ".exe" } else { "" };
        format!("server/dist/log-analytics-tantivy-{platform}{ext}")
    }

    #[test]
    fn plugin_json_is_laid_out() {
        assert_eq!(layout(&manifest()).unwrap(), PLUGIN_JSON, "plugin.json is not laid out with two space indentation");
    }

    #[test]
    fn identity_matches_the_crate() {
        let m = manifest();
        assert_eq!(m["id"], "com.nginxui.log-analytics-tantivy");
        assert_eq!(m["version"], env!("CARGO_PKG_VERSION"), "plugin.json and Cargo.toml name different versions");
        assert_eq!(m["conflicts"], serde_json::json!(["com.nginxui.log-analytics"]));
        assert_eq!(m["capabilities"], serde_json::json!(["http"]));
        assert_eq!(m["server"]["lifecycle"], "resident");
        for (platform, path) in m["server"]["executables"].as_object().unwrap() {
            assert_eq!(path, &executable_path(platform), "executable of {platform}");
        }
    }

    #[test]
    fn user_facing_text_names_no_engine() {
        // The id and the file names may name the engine, what people read may not
        let m = manifest();
        let mut shown = vec![
            m["name"].as_str().unwrap_or_default().to_owned(),
            m["description"].as_str().unwrap_or_default().to_owned(),
        ];
        shown.extend(texts(&m["permission_reasons"]));
        for translated in m["i18n"].as_object().into_iter().flat_map(|o| o.values()) {
            shown.extend(texts(translated));
            shown.extend(texts(&translated["permission_reasons"]));
        }
        for field in m["settings_schema"]["settings"].as_array().into_iter().flatten() {
            shown.extend(texts(field));
        }
        let text = shown.join("\n").to_lowercase();
        for word in ["tantivy", "bleve", "lucene"] {
            assert!(!text.contains(word), "{word} in the manifest");
        }
    }

    #[test]
    fn permission_reasons_name_requested_permissions() {
        let m = manifest();
        let permissions: Vec<&str> = m["permissions"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let mut blocks = vec![("permission_reasons".to_owned(), &m["permission_reasons"])];
        for (locale, translated) in m["i18n"].as_object().into_iter().flatten() {
            blocks.push((format!("i18n.{locale}.permission_reasons"), &translated["permission_reasons"]));
        }
        for (where_, reasons) in blocks {
            for permission in reasons.as_object().into_iter().flat_map(|o| o.keys()) {
                assert!(
                    permissions.contains(&permission.as_str()),
                    "{where_} explains {permission}, which is not requested"
                );
            }
        }
    }

    #[test]
    fn narrowing_keeps_one_platform_and_everything_else() {
        let m = manifest();
        for (platform, path) in m["server"]["executables"].as_object().unwrap() {
            let narrowed: Value = serde_json::from_str(&narrow(PLUGIN_JSON, platform).unwrap()).unwrap();
            let exes = narrowed["server"]["executables"].as_object().unwrap();
            assert_eq!(exes.len(), 1);
            assert_eq!(&exes[platform], path);

            let mut restored = narrowed.clone();
            restored["server"]["executables"] = m["server"]["executables"].clone();
            assert_eq!(layout(&restored).unwrap(), PLUGIN_JSON, "{platform} changes more than the executables");
        }
        assert!(narrow(PLUGIN_JSON, "plan9-amd64").is_err());
    }

    /// The webapp build writes its paths and the shared libraries it expects
    /// in a fragment. plugin.json has to name the same.
    #[test]
    fn manifest_matches_the_webapp_fragment() {
        let root = env!("CARGO_MANIFEST_DIR");
        let Ok(text) = std::fs::read_to_string(format!("{root}/webapp/dist/manifest.webapp.json")) else {
            return;
        };
        let fragment: Value = serde_json::from_str(&text).unwrap();
        let webapp = &manifest()["webapp"];
        for key in ["bundle_path", "style_path", "chunks", "shared"] {
            assert_eq!(
                webapp[key], fragment[key],
                "webapp.{key} differs from the webapp build, copy it into plugin.json"
            );
        }
    }
}
