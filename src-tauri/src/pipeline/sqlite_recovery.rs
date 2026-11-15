//! Recovery of a task-owned, mutable COLMAP database. Immutable source snapshots
//! must never use this helper. SQLite, not the application, owns journal replay
//! and WAL cleanup; a nonempty sidecar alone is not evidence of corruption.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use rusqlite::{Connection, ErrorCode, OpenFlags};
use tokio_util::sync::CancellationToken;

use crate::{
    error::{Result, SplatError},
    reconstruction::direct_input::database_read_error,
};

const LOCK_DEADLINE: Duration = Duration::from_secs(2);
const LOCK_POLL: Duration = Duration::from_millis(25);

fn cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn wait_for_lock(started: Instant, cancel: &CancellationToken) -> Result<bool> {
    cancelled(cancel)?;
    let remaining = LOCK_DEADLINE.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Ok(false);
    }
    std::thread::sleep(remaining.min(LOCK_POLL));
    cancelled(cancel)?;
    Ok(true)
}

/// Returns a connection retaining SQLite's exclusive lock through validation.
/// It never creates a missing database, changes its schema, disables journaling,
/// or removes a sidecar using filesystem operations.
pub(super) fn settle(path: &Path, cancel: &CancellationToken) -> Result<Connection> {
    cancelled(cancel)?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(database_read_error)?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(database_read_error)?;
    // Set this before the first database read. In WAL mode this acquires the
    // database-file lock as well, so a checkpoint cannot race another writer.
    let mode: String = connection
        .query_row("PRAGMA main.locking_mode=EXCLUSIVE", [], |row| row.get(0))
        .map_err(database_read_error)?;
    if !mode.eq_ignore_ascii_case("exclusive") {
        return Err(SplatError::Io(std::io::Error::other(
            "SQLite could not establish exclusive recovery mode",
        )));
    }
    let started = Instant::now();
    loop {
        cancelled(cancel)?;
        match connection.execute_batch("BEGIN EXCLUSIVE") {
            Ok(()) => break,
            Err(error) if busy(&error) && wait_for_lock(started, cancel)? => {}
            Err(error) => return Err(database_read_error(error)),
        }
    }
    cancelled(cancel)?;
    // No application data is changed. Exclusive locking mode retains the lock
    // after this empty transaction, permitting a checkpoint outside a transaction.
    connection
        .execute_batch("COMMIT")
        .map_err(database_read_error)?;
    checkpoint(&connection, started, cancel)?;
    Ok(connection)
}

fn checkpoint(connection: &Connection, started: Instant, cancel: &CancellationToken) -> Result<()> {
    loop {
        cancelled(cancel)?;
        let result = connection.query_row("PRAGMA main.wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        });
        match result {
            Ok((0, pages, checkpointed)) if pages == checkpointed => break,
            Ok(_) if wait_for_lock(started, cancel)? => {}
            Ok(_) => {
                return Err(SplatError::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "COLMAP database is in use; SQLite checkpoint could not complete",
                )))
            }
            Err(error) if busy(&error) && wait_for_lock(started, cancel)? => {}
            Err(error) => return Err(database_read_error(error)),
        }
    }
    cancelled(cancel)?;
    Ok(())
}

fn owned_database_family(root: &Path, path: &Path, must_exist: bool) -> Result<bool> {
    use super::artifacts::{reject_shared_recovery_file, validate_recovery_path};
    let relative = path.strip_prefix(root).map_err(|_| {
        SplatError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Database copy is outside the task's private work directory",
        ))
    })?;
    if relative.components().any(|part| {
        !matches!(part, std::path::Component::Normal(_))
            || part.as_os_str() == "reshoot-snapshot-v3"
    }) {
        return Err(SplatError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Immutable source snapshots cannot be opened for database copy",
        )));
    }
    let canonical_root = std::fs::canonicalize(root)?;
    let exists = match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_recovery_path(root, path)?;
            if !metadata.file_type().is_file()
                || !std::fs::canonicalize(path)?.starts_with(&canonical_root)
            {
                return Err(SplatError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Unsafe database copy target",
                )));
            }
            reject_shared_recovery_file(path)?;
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !must_exist => {
            let parent = path
                .parent()
                .ok_or_else(|| SplatError::Process("Invalid database copy target".into()))?;
            validate_recovery_path(root, parent)?;
            if !std::fs::canonicalize(parent)?.starts_with(&canonical_root) {
                return Err(SplatError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Unsafe database copy directory",
                )));
            }
            false
        }
        Err(error) => return Err(error.into()),
    };
    for suffix in ["-wal", "-journal", "-shm"] {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        let sidecar = std::path::PathBuf::from(value);
        match std::fs::symlink_metadata(&sidecar) {
            Ok(metadata) => {
                if !exists || !metadata.file_type().is_file() {
                    return Err(SplatError::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "Unsafe database copy sidecar",
                    )));
                }
                validate_recovery_path(root, &sidecar)?;
                reject_shared_recovery_file(&sidecar)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(exists)
}

/// Copy only private, mutable work databases using SQLite's backup transaction.
/// Existing destination journals are recovered by SQLite, never overwritten by
/// copying a database.db alone. Immutable snapshots must use read-only copying.
pub(super) fn copy_owned_database(
    root: &Path,
    source: &Path,
    target: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    cancelled(cancel)?;
    owned_database_family(root, source, true)?;
    let target_exists = owned_database_family(root, target, false)?;
    if source == target
        || (target_exists && std::fs::canonicalize(source)? == std::fs::canonicalize(target)?)
    {
        return Err(SplatError::Process(
            "Database copy source and target are identical".into(),
        ));
    }
    let source_db = settle(source, cancel)?;
    let source_identity = super::artifacts::read_database_summary(&source_db, cancel)?;
    if !target_exists {
        // Claim the pathname without CREATE/overwrite in SQLite's open flags.
        let new_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        new_file.sync_all()?;
    }
    let mut target_db = settle(target, cancel)?;
    if target_exists {
        // Bad destinations are not disguised as a successful copy. The caller
        // owns any durable whole-attempt archive needed before a fresh attempt.
        super::artifacts::read_database_summary(&target_db, cancel)?;
    }
    {
        use rusqlite::backup::{Backup, StepResult};
        let backup = Backup::new(&source_db, &mut target_db).map_err(database_read_error)?;
        let mut wait_started = None;
        loop {
            cancelled(cancel)?;
            match backup.step(128).map_err(database_read_error)? {
                StepResult::Done => break,
                StepResult::More => wait_started = None,
                StepResult::Busy | StepResult::Locked => {
                    let started = *wait_started.get_or_insert_with(Instant::now);
                    if !wait_for_lock(started, cancel)? {
                        return Err(SplatError::Io(std::io::Error::new(
                            std::io::ErrorKind::WouldBlock,
                            "COLMAP database is in use; SQLite backup could not complete",
                        )));
                    }
                }
                _ => return Err(SplatError::Process("Unexpected SQLite backup state".into())),
            }
        }
    }
    if super::artifacts::read_database_summary(&target_db, cancel)? != source_identity {
        return Err(SplatError::Process(
            "SQLite database copy failed semantic verification".into(),
        ));
    }
    checkpoint(&target_db, Instant::now(), cancel)?;
    target_db
        .close()
        .map_err(|(_, error)| database_read_error(error))?;
    source_db
        .close()
        .map_err(|(_, error)| database_read_error(error))?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(target)?
        .sync_all()?;
    cancelled(cancel)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn recovery_never_creates_missing_database() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing.db");
        assert!(settle(&path, &CancellationToken::new()).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn persist_journal_and_wal_use_native_sqlite_recovery() {
        for journal in ["PERSIST", "WAL"] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("database.db");
            let original = Connection::open(&path).unwrap();
            original.execute_batch(&format!("PRAGMA journal_mode={journal}; CREATE TABLE proof(value INTEGER); INSERT INTO proof VALUES(42)")).unwrap();
            drop(original);
            if journal == "PERSIST" {
                assert!(
                    std::fs::metadata(root.path().join("database.db-journal"))
                        .unwrap()
                        .len()
                        > 0
                );
            }
            let settled = settle(&path, &CancellationToken::new()).unwrap();
            assert_eq!(
                settled
                    .query_row("SELECT value FROM proof", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                42
            );
            assert_eq!(
                settled
                    .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "ok"
            );
        }
    }

    #[test]
    fn active_writer_is_not_recovered_or_classified_as_corruption() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("database.db");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE proof(value INTEGER); BEGIN IMMEDIATE; INSERT INTO proof VALUES(42)").unwrap();
        let result = settle(&path, &CancellationToken::new());
        assert!(
            matches!(result, Err(SplatError::Io(ref error)) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        assert!(!writer.is_autocommit());
        writer.execute_batch("ROLLBACK").unwrap();
        drop(writer);
        assert!(settle(&path, &CancellationToken::new()).is_ok());
    }

    #[test]
    fn cancellation_does_not_recover_database() {
        let root = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(
            settle(&root.path().join("absent.db"), &token),
            Err(SplatError::Cancelled)
        ));
    }

    /// An actual isolated process exits without running SQLite destructors.
    /// This does not kill an application or access a user project/database.
    pub(crate) fn crash_fixture(path: &Path, mode: &str) {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pipeline::sqlite_recovery::tests::crashed_writer_child",
                "--ignored",
                "--nocapture",
            ])
            .env("OOOSPLAT_SQLITE_RECOVERY_TEST_PATH", path)
            .env("OOOSPLAT_SQLITE_RECOVERY_TEST_MODE", mode)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77));
    }

    #[test]
    #[ignore = "isolated SQLite crash fixture, invoked by its parent test"]
    fn crashed_writer_child() {
        let path =
            std::env::var_os("OOOSPLAT_SQLITE_RECOVERY_TEST_PATH").expect("isolated fixture path");
        let mode = std::env::var("OOOSPLAT_SQLITE_RECOVERY_TEST_MODE").unwrap();
        let db = Connection::open(path).unwrap();
        if mode == "layout" {
            db.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA user_version=17",
            )
            .unwrap();
        } else if mode == "wal" {
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE proof(value INTEGER); INSERT INTO proof VALUES(42); BEGIN IMMEDIATE; INSERT INTO proof VALUES(99)").unwrap();
        } else if mode == "colmap-wal" {
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; UPDATE images SET name=name || '-wal'; PRAGMA user_version=18").unwrap();
        } else {
            assert_eq!(mode, "journal");
            db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA cache_size=1; CREATE TABLE proof(value INTEGER); INSERT INTO proof VALUES(42); CREATE TABLE spill(data BLOB); BEGIN IMMEDIATE; UPDATE proof SET value=99; INSERT INTO spill VALUES(zeroblob(65536))").unwrap();
        }
        std::process::exit(77);
    }

    #[test]
    fn crash_left_wal_and_hot_journal_recover_committed_data_only() {
        for mode in ["wal", "journal"] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("database.db");
            crash_fixture(&path, mode);
            let suffix = if mode == "wal" { "-wal" } else { "-journal" };
            assert!(
                std::fs::metadata(root.path().join(format!("database.db{suffix}")))
                    .unwrap()
                    .len()
                    > 0
            );
            let db = settle(&path, &CancellationToken::new()).unwrap();
            let values = db
                .prepare("SELECT value FROM proof")
                .unwrap()
                .query_map([], |row| row.get::<_, i64>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(values, [42], "{mode}");
        }
    }

    #[test]
    fn waiting_for_an_active_writer_is_cancelled_without_modifying_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("database.db");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "CREATE TABLE proof(value); BEGIN EXCLUSIVE; INSERT INTO proof VALUES(42)",
            )
            .unwrap();
        let token = CancellationToken::new();
        let cancellation = token.clone();
        let cancellation_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(75));
            cancellation.cancel();
        });
        let started = Instant::now();
        assert!(matches!(settle(&path, &token), Err(SplatError::Cancelled)));
        cancellation_thread.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!writer.is_autocommit());
    }

    fn colmap_database(path: &Path, image: &str) {
        let db = Connection::open(path).unwrap();
        db.execute_batch("CREATE TABLE cameras(camera_id INTEGER PRIMARY KEY,model INTEGER,width INTEGER,height INTEGER,params BLOB); CREATE TABLE images(image_id INTEGER PRIMARY KEY,name TEXT,camera_id INTEGER); CREATE TABLE keypoints(image_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); CREATE TABLE descriptors(image_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); CREATE TABLE matches(pair_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); CREATE TABLE two_view_geometries(pair_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); INSERT INTO cameras VALUES(1,1,16,16,X'0000000000002440000000000000244000000000000020400000000000002040'); INSERT INTO keypoints VALUES(1,1,2,X'0000803F0000803F'); INSERT INTO descriptors VALUES(1,1,128,zeroblob(128));").unwrap();
        db.execute("INSERT INTO images VALUES(1,?1,1)", [image])
            .unwrap();
    }

    #[test]
    fn backup_recovers_source_and_target_wal_instead_of_overwriting_only_the_main_file() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.db");
        let target = root.path().join("target.db");
        colmap_database(&source, "new");
        colmap_database(&target, "stale");
        crash_fixture(&source, "colmap-wal");
        crash_fixture(&target, "colmap-wal");
        let token = CancellationToken::new();
        copy_owned_database(root.path(), &source, &target, &token).unwrap();
        for path in [&source, &target] {
            let db = Connection::open(path).unwrap();
            assert_eq!(
                db.query_row("SELECT name FROM images", [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "new-wal"
            );
            assert_eq!(
                db.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "ok"
            );
        }
        // Reapplying an interrupted publish is idempotent, not raw file copy.
        copy_owned_database(root.path(), &source, &target, &token).unwrap();
    }

    #[test]
    fn backup_creates_only_its_new_destination_and_refuses_corrupt_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.db");
        let target = root.path().join("target.db");
        colmap_database(&source, "new");
        copy_owned_database(root.path(), &source, &target, &CancellationToken::new()).unwrap();
        let invalid = root.path().join("invalid.db");
        std::fs::write(&invalid, b"corrupted target must remain evidence").unwrap();
        let before = std::fs::read(&invalid).unwrap();
        assert!(
            copy_owned_database(root.path(), &source, &invalid, &CancellationToken::new()).is_err()
        );
        assert_eq!(std::fs::read(&invalid).unwrap(), before);
    }

    #[test]
    fn backup_refuses_active_source_or_target_writer_without_rolling_it_back() {
        for locked_source in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("source.db");
            let target = root.path().join("target.db");
            colmap_database(&source, "new");
            colmap_database(&target, "old");
            let writer = Connection::open(if locked_source { &source } else { &target }).unwrap();
            writer.execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE; UPDATE images SET name='uncommitted'").unwrap();
            let error =
                copy_owned_database(root.path(), &source, &target, &CancellationToken::new())
                    .unwrap_err();
            assert_eq!(
                error.io_error().unwrap().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert!(!writer.is_autocommit());
            writer.execute_batch("ROLLBACK").unwrap();
            drop(writer);
            let target_db = Connection::open(&target).unwrap();
            assert_eq!(
                target_db
                    .query_row("SELECT name FROM images", [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "old"
            );
        }
    }

    #[test]
    fn backup_wait_can_be_cancelled_without_changing_an_active_target() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.db");
        let target = root.path().join("target.db");
        colmap_database(&source, "new");
        colmap_database(&target, "old");
        let writer = Connection::open(&target).unwrap();
        writer
            .execute_batch("BEGIN EXCLUSIVE; UPDATE images SET name='active'")
            .unwrap();
        let cancel = CancellationToken::new();
        let pending = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(75));
            pending.cancel();
        });
        assert!(matches!(
            copy_owned_database(root.path(), &source, &target, &cancel),
            Err(SplatError::Cancelled)
        ));
        thread.join().unwrap();
        assert!(!writer.is_autocommit());
        assert_eq!(
            writer
                .query_row("SELECT name FROM images", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "active"
        );
    }

    #[test]
    fn backup_rejects_immutable_snapshots_and_shared_targets() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.db");
        let linked = root.path().join("linked.db");
        let snapshot = root.path().join("reshoot-snapshot-v3");
        std::fs::create_dir(&snapshot).unwrap();
        let snapshot_db = snapshot.join("database.db");
        colmap_database(&source, "new");
        colmap_database(&snapshot_db, "frozen");
        std::fs::hard_link(&snapshot_db, &linked).unwrap();
        let before = std::fs::read(&snapshot_db).unwrap();
        assert!(
            copy_owned_database(root.path(), &source, &linked, &CancellationToken::new()).is_err()
        );
        assert!(copy_owned_database(
            root.path(),
            &snapshot_db,
            &root.path().join("missing.db"),
            &CancellationToken::new()
        )
        .is_err());
        assert!(!root.path().join("missing.db").exists());
        assert_eq!(std::fs::read(&snapshot_db).unwrap(), before);
    }
}
