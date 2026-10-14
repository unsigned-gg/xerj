use std::path::{Path, PathBuf};

/// Filesystem and network permissions for a jailed child process.
///
/// Default policy is deny-all: no extra filesystem paths beyond the OS runtime
/// roots, and no outbound TCP except `localhost`.
#[derive(Debug, Clone)]
pub struct AccessRules {
    pub read_paths: Vec<PathBuf>,
    pub write_paths: Vec<PathBuf>,
    pub allow_localhost_tcp: bool,
}

impl Default for AccessRules {
    fn default() -> Self {
        Self {
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            allow_localhost_tcp: true,
        }
    }
}

impl AccessRules {
    /// Add a read-only path (plus execute for binaries inside it).
    pub fn with_read(mut self, path: impl AsRef<Path>) -> Self {
        self.read_paths.push(path.as_ref().to_path_buf());
        self
    }

    /// Add a read-write path (workspace directory, scratch root, etc.).
    pub fn with_write(mut self, path: impl AsRef<Path>) -> Self {
        self.write_paths.push(path.as_ref().to_path_buf());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_have_empty_paths_and_allow_localhost() {
        let r = AccessRules::default();
        assert!(r.read_paths.is_empty());
        assert!(r.write_paths.is_empty());
        assert!(r.allow_localhost_tcp);
    }

    #[test]
    fn builder_methods_append_in_order() {
        let r = AccessRules::default()
            .with_read("/usr")
            .with_read("/etc")
            .with_write("/tmp/work");
        assert_eq!(
            r.read_paths,
            vec![PathBuf::from("/usr"), PathBuf::from("/etc")]
        );
        assert_eq!(r.write_paths, vec![PathBuf::from("/tmp/work")]);
    }
}
