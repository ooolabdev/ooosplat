//! Registry ownership is distinct from the shared engine execution lock.
//! Loading a registry repairs interrupted records, so it must never happen
//! from a second CLI while the first still owns its live TaskService.
use std::{fs::File, path::Path};

pub fn claim(root: &Path) -> super::Result<File> {
    std::fs::create_dir_all(root)?;
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("owner.lock"))?;
    owner.try_lock().map_err(|error| {
        super::ServiceError::new(
            "TASK_BUSY",
            format!("CLI task records are already in use: {error}"),
        )
    })?;
    Ok(owner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_registry_child_probe() {
        let Some(root) = std::env::var_os("OOOSPLAT_TEST_CLI_REGISTRY") else {
            return;
        };
        match claim(Path::new(&root)) {
            Ok(_owner) => println!("CLI_OWNER_ACQUIRED"),
            Err(error) => {
                assert_eq!(error.code, "TASK_BUSY");
                println!("CLI_OWNER_BUSY");
            }
        }
    }

    #[test]
    fn second_process_cannot_repair_live_registry_and_lock_releases() {
        let root = tempfile::tempdir().unwrap();
        let records = root.path().join("tasks.json");
        std::fs::write(&records, b"live registry must not be loaded").unwrap();
        let original = std::fs::read(&records).unwrap();
        let probe = || {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tasks::cli_guard::tests::cli_registry_child_probe",
                    "--nocapture",
                ])
                .env("OOOSPLAT_TEST_CLI_REGISTRY", root.path())
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        let owner = claim(root.path()).unwrap();
        assert!(probe().contains("CLI_OWNER_BUSY"));
        assert_eq!(std::fs::read(&records).unwrap(), original);
        drop(owner);
        assert!(probe().contains("CLI_OWNER_ACQUIRED"));
        assert_eq!(std::fs::read(records).unwrap(), original);
    }
}
