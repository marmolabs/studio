use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StudioPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl StudioPaths {
    pub fn resolve(home_override: Option<PathBuf>) -> Self {
        let home = home_override.or_else(|| std::env::var_os("STUDIO_HOME").map(PathBuf::from));
        if let Some(home) = home {
            Self {
                config_dir: home.clone(),
                data_dir: home.join("data"),
            }
        } else {
            Self {
                config_dir: dirs::config_dir().unwrap_or_default().join("studio"),
                data_dir: dirs::data_dir().unwrap_or_default().join("studio"),
            }
        }
    }

    pub fn resolve_path(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.config_dir.join(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_override_sets_config_and_data() {
        let paths = StudioPaths::resolve(Some(PathBuf::from("/custom/home")));
        assert_eq!(paths.config_dir, PathBuf::from("/custom/home"));
        assert_eq!(paths.data_dir, PathBuf::from("/custom/home/data"));
    }

    #[test]
    fn relative_paths_resolve_against_config_dir() {
        let paths = StudioPaths::resolve(Some(PathBuf::from("/custom/home")));
        assert_eq!(
            paths.resolve_path("packs/acme"),
            PathBuf::from("/custom/home/packs/acme")
        );
    }

    #[test]
    fn absolute_paths_preserved() {
        let paths = StudioPaths::resolve(Some(PathBuf::from("/custom/home")));
        assert_eq!(paths.resolve_path("/abs/path"), PathBuf::from("/abs/path"));
    }
}
