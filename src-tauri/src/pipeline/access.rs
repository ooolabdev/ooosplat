//! Current-caller permissions for external input reads. Never persisted in a
//! work order: a saved path is provenance, not a reusable authorization grant.
use crate::{
    error::{Result, SplatError},
    pipeline::inputs::{GenerationInputs, InputSource},
};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct InvocationAccess {
    input_roots: Vec<PathBuf>,
}

impl InvocationAccess {
    pub fn restricted(input_roots: Vec<PathBuf>) -> Self {
        Self { input_roots }
    }

    /// Only an executor that has bound its owned project may add this root.
    /// Canonical per-file checks below still reject redirects outside it.
    pub(crate) fn with_owned_project(mut self, project: &Path) -> Self {
        if let Ok(root) = std::fs::canonicalize(project) {
            if !self.input_roots.contains(&root) {
                self.input_roots.push(root);
            }
        }
        self
    }

    pub fn authorize(&self, path: &Path) -> Result<()> {
        crate::tasks::authorize_input(path, &self.input_roots)
            .map(|_| ())
            .map_err(|error| SplatError::Process(format!("{}: {}", error.code, error.message)))
    }

    /// Check roots before enumerating or parsing their contents, then check
    /// members independently so redirects do not inherit a directory grant.
    pub fn authorize_inputs(&self, inputs: &GenerationInputs) -> Result<()> {
        match inputs {
            GenerationInputs::Materials { items } => {
                for item in items {
                    match &item.source {
                        InputSource::Images { paths } => {
                            for path in paths {
                                self.authorize(path)?;
                            }
                        }
                        source => self.authorize(source.path())?,
                    }
                }
            }
            GenerationInputs::Colmap { input } => {
                self.authorize_model(&input.model)?;
                self.authorize(&input.images)?;
                if let Some(path) = &input.database {
                    self.authorize(if path.exists() {
                        path
                    } else {
                        path.parent().unwrap_or(path)
                    })?;
                }
            }
        }
        for path in inputs.authorized_paths()? {
            self.authorize(&path)?;
        }
        Ok(())
    }

    pub fn authorize_image_directory(&self, path: &Path) -> Result<()> {
        self.authorize(path)?;
        for image in crate::video::list_images(path)? {
            self.authorize(&image)?;
        }
        Ok(())
    }

    pub fn authorize_model(&self, path: &Path) -> Result<()> {
        self.authorize(path)?;
        for name in [
            "cameras.bin",
            "images.bin",
            "points3D.bin",
            "cameras.txt",
            "images.txt",
            "points3D.txt",
        ] {
            let file = path.join(name);
            if file.exists() {
                self.authorize(&file)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_belongs_to_current_caller_and_does_not_require_unused_sources() {
        let root = tempfile::tempdir().unwrap();
        let allowed = std::fs::canonicalize(root.path()).unwrap();
        let input = root.path().join("input.mov");
        std::fs::write(&input, b"input").unwrap();
        let caller = InvocationAccess::restricted(vec![allowed]);
        caller.authorize(&input).unwrap();
        assert!(InvocationAccess::restricted(vec![])
            .authorize(&input)
            .unwrap_err()
            .to_string()
            .contains("PATH_NOT_AUTHORIZED"));
        std::fs::remove_file(&input).unwrap();
        // Keeping the context is harmless; only a real external read checks it.
        assert!(caller
            .authorize(&input)
            .unwrap_err()
            .to_string()
            .contains("INVALID_PATH"));
    }

    #[test]
    fn owned_project_grant_does_not_authorize_referenced_external_files() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let owned = project.join("input.mov");
        let external = root.path().join("external.mov");
        std::fs::write(&owned, b"owned").unwrap();
        std::fs::write(&external, b"outside").unwrap();
        let caller = InvocationAccess::restricted(vec![]);
        assert!(caller.authorize(&owned).is_err());
        let bound = caller.with_owned_project(&project);
        bound.authorize(&owned).unwrap();
        assert!(bound
            .authorize(&external)
            .unwrap_err()
            .to_string()
            .contains("PATH_NOT_AUTHORIZED"));
    }

    #[test]
    fn owned_project_symlink_does_not_inherit_authorization() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let external = root.path().join("external.png");
        std::fs::write(&external, b"outside").unwrap();
        let redirected = project.join("redirected.png");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&external, &redirected);
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&external, &redirected);
        if let Err(error) = linked {
            assert!(
                error.raw_os_error() == Some(1314)
                    || matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
                    ),
                "{error}"
            );
            eprintln!("Skipping symlink fixture: {error}");
            return;
        }
        let bound = InvocationAccess::restricted(vec![]).with_owned_project(&project);
        assert!(bound
            .authorize(&redirected)
            .unwrap_err()
            .to_string()
            .contains("PATH_NOT_AUTHORIZED"));
    }
}
