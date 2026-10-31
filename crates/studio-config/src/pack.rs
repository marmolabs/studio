//! A pack is a folder with a `pack.toml` and optional `hires/`, `rules/`,
//! `skills/` and a `project.toml` template for a project's `.studio.toml`.

use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
}

/// Reads and checks `<dir>/pack.toml`.
pub fn load_manifest(dir: &Path) -> Result<PackManifest, String> {
    let path = dir.join("pack.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest: PackManifest =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = &manifest.name;
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(format!(
            "pack names use letters, digits, - and _ only: {name}"
        ));
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn reads_a_manifest() {
        let tmp = TempDir::new().unwrap();
        assert!(load_manifest(tmp.path()).is_err());
        let write = |text: &str| fs::write(tmp.path().join("pack.toml"), text).unwrap();
        write("name = \"p\"\ndescription = \"d\"\nversion = \"1\"\n");
        let manifest = load_manifest(tmp.path()).unwrap();
        assert_eq!(
            (manifest.name.as_str(), manifest.version.as_str()),
            ("p", "1")
        );
        write("name = \"bad name\"\n");
        assert!(load_manifest(tmp.path()).is_err());
        write("name = \"p\"\nextra = 1\n");
        assert!(load_manifest(tmp.path()).is_err());
    }
}
