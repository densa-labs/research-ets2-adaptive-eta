use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use adaptive_eta_core::{
    CALIBRATION_MODEL_VERSION, CalibrationSnapshot, EstimatorState, SnapshotValidationError,
};
use serde::{Deserialize, Serialize};

pub const PROFILE_SCHEMA_VERSION: u32 = 1;
const PRODUCT_ID: &str = "adaptive-eta";
const GAME_ID: &str = "ets2";
const PROFILE_FILE: &str = "default.json";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathEnvironment {
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    MissingHome,
    MissingLocalAppData,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidProfileReason {
    Malformed,
    ProductMismatch,
    GameMismatch,
    Calibration(SnapshotValidationError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteStage {
    CreateDirectory,
    CreateTemporary,
    WriteTemporary,
    FlushTemporary,
    AtomicReplace,
    SyncDirectory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileDiagnostic {
    ProfileLoaded,
    ProfileNotFound,
    ProfileSaved,
    ProfileInvalid(InvalidProfileReason),
    UnsupportedSchemaVersion {
        found: u32,
    },
    UnsupportedModelVersion {
        found: u32,
    },
    ProfileReadFailed {
        kind: io::ErrorKind,
    },
    ProfileWriteFailed {
        stage: WriteStage,
        kind: io::ErrorKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProfileState {
    Fresh,
    Loaded(CalibrationSnapshot),
    Degraded,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadOutcome {
    pub state: ProfileState,
    pub diagnostic: ProfileDiagnostic,
    /// False protects malformed, unsupported, or unreadable evidence from an
    /// automatic downgrade/overwrite during this process.
    pub writes_allowed: bool,
}

#[derive(Clone, Debug)]
pub struct ProfileStore {
    path: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileDocument {
    schema_version: u32,
    product: String,
    game: String,
    calibration: CalibrationDocument,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CalibrationDocument {
    model_version: u32,
    log_factor: f64,
    evidence_distance_km: f64,
    sample_count: u64,
}

impl From<CalibrationSnapshot> for CalibrationDocument {
    fn from(snapshot: CalibrationSnapshot) -> Self {
        Self {
            model_version: snapshot.model_version,
            log_factor: snapshot.log_factor,
            evidence_distance_km: snapshot.evidence_distance_km,
            sample_count: snapshot.sample_count,
        }
    }
}

impl From<CalibrationDocument> for CalibrationSnapshot {
    fn from(document: CalibrationDocument) -> Self {
        Self {
            model_version: document.model_version,
            log_factor: document.log_factor,
            evidence_distance_km: document.evidence_distance_km,
            sample_count: document.sample_count,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchemaProbe {
    schema_version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelProbe {
    calibration: ModelVersionProbe,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelVersionProbe {
    model_version: u32,
}

impl ProfileStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn load(&self) -> LoadOutcome {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return LoadOutcome {
                    state: ProfileState::Fresh,
                    diagnostic: ProfileDiagnostic::ProfileNotFound,
                    writes_allowed: true,
                };
            }
            Err(error) => {
                return LoadOutcome {
                    state: ProfileState::Degraded,
                    diagnostic: ProfileDiagnostic::ProfileReadFailed { kind: error.kind() },
                    writes_allowed: false,
                };
            }
        };
        let schema: SchemaProbe = match serde_json::from_slice(&bytes) {
            Ok(schema) => schema,
            Err(_) => {
                return invalid_outcome(InvalidProfileReason::Malformed);
            }
        };
        if schema.schema_version != PROFILE_SCHEMA_VERSION {
            return LoadOutcome {
                state: ProfileState::Degraded,
                diagnostic: ProfileDiagnostic::UnsupportedSchemaVersion {
                    found: schema.schema_version,
                },
                writes_allowed: false,
            };
        }
        let model: ModelProbe = match serde_json::from_slice(&bytes) {
            Ok(model) => model,
            Err(_) => return invalid_outcome(InvalidProfileReason::Malformed),
        };
        if model.calibration.model_version != CALIBRATION_MODEL_VERSION {
            return LoadOutcome {
                state: ProfileState::Degraded,
                diagnostic: ProfileDiagnostic::UnsupportedModelVersion {
                    found: model.calibration.model_version,
                },
                writes_allowed: false,
            };
        }
        let document: ProfileDocument = match serde_json::from_slice(&bytes) {
            Ok(document) => document,
            Err(_) => return invalid_outcome(InvalidProfileReason::Malformed),
        };
        if document.product != PRODUCT_ID {
            return invalid_outcome(InvalidProfileReason::ProductMismatch);
        }
        if document.game != GAME_ID {
            return invalid_outcome(InvalidProfileReason::GameMismatch);
        }
        let snapshot = CalibrationSnapshot::from(document.calibration);
        match EstimatorState::from_snapshot(snapshot) {
            Ok(_) => LoadOutcome {
                state: ProfileState::Loaded(snapshot),
                diagnostic: ProfileDiagnostic::ProfileLoaded,
                writes_allowed: true,
            },
            Err(SnapshotValidationError::UnsupportedModelVersion { found }) => LoadOutcome {
                state: ProfileState::Degraded,
                diagnostic: ProfileDiagnostic::UnsupportedModelVersion { found },
                writes_allowed: false,
            },
            Err(error) => invalid_outcome(InvalidProfileReason::Calibration(error)),
        }
    }

    /// Atomically replaces the profile with a validated snapshot.
    ///
    /// # Errors
    ///
    /// Returns a typed stage and I/O kind. The previous final profile remains
    /// untouched unless replacement succeeds.
    pub fn save(&self, snapshot: CalibrationSnapshot) -> Result<(), ProfileDiagnostic> {
        EstimatorState::from_snapshot(snapshot).map_err(|error| {
            ProfileDiagnostic::ProfileInvalid(InvalidProfileReason::Calibration(error))
        })?;
        let document = ProfileDocument {
            schema_version: PROFILE_SCHEMA_VERSION,
            product: PRODUCT_ID.to_owned(),
            game: GAME_ID.to_owned(),
            calibration: snapshot.into(),
        };
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|_| ProfileDiagnostic::ProfileInvalid(InvalidProfileReason::Malformed))?;
        let parent = self
            .path
            .parent()
            .ok_or(ProfileDiagnostic::ProfileWriteFailed {
                stage: WriteStage::CreateDirectory,
                kind: io::ErrorKind::InvalidInput,
            })?;
        create_private_directories(parent).map_err(|error| {
            ProfileDiagnostic::ProfileWriteFailed {
                stage: WriteStage::CreateDirectory,
                kind: error.kind(),
            }
        })?;

        let temporary = temporary_path(&self.path);
        let result = write_and_replace(&temporary, &self.path, parent, &bytes);
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn invalid_outcome(reason: InvalidProfileReason) -> LoadOutcome {
    LoadOutcome {
        state: ProfileState::Degraded,
        diagnostic: ProfileDiagnostic::ProfileInvalid(reason),
        writes_allowed: false,
    }
}

/// Builds the default ETS2 profile path from explicit platform directory inputs.
///
/// # Errors
///
/// Returns an error when the selected platform's required base directory is absent.
pub fn resolve_profile_path(
    platform: Platform,
    paths: &PathEnvironment,
) -> Result<PathBuf, PathError> {
    let base = match platform {
        Platform::Windows => paths
            .local_app_data
            .as_ref()
            .ok_or(PathError::MissingLocalAppData)?
            .join("Adaptive ETA"),
        Platform::MacOs => paths
            .home
            .as_ref()
            .ok_or(PathError::MissingHome)?
            .join("Library")
            .join("Application Support")
            .join("Adaptive ETA"),
        Platform::Linux => paths
            .xdg_data_home
            .clone()
            .or_else(|| {
                paths
                    .home
                    .as_ref()
                    .map(|home| home.join(".local").join("share"))
            })
            .ok_or(PathError::MissingHome)?
            .join("adaptive-eta"),
    };
    Ok(base.join("profiles").join(GAME_ID).join(PROFILE_FILE))
}

/// Resolves the current OS user's default ETS2 profile path without creating it.
///
/// # Errors
///
/// Returns an error when the platform's required per-user base directory is unavailable.
pub fn resolve_current_profile_path() -> Result<PathBuf, PathError> {
    #[cfg(target_os = "windows")]
    let path = dirs::data_local_dir()
        .ok_or(PathError::MissingLocalAppData)?
        .join("Adaptive ETA");
    #[cfg(target_os = "macos")]
    let path = dirs::data_dir()
        .ok_or(PathError::MissingHome)?
        .join("Adaptive ETA");
    #[cfg(all(unix, not(target_os = "macos")))]
    let path = dirs::data_dir()
        .ok_or(PathError::MissingHome)?
        .join("adaptive-eta");
    #[cfg(not(any(unix, target_os = "windows")))]
    compile_error!("profile path resolution is not implemented for this platform");
    Ok(path.join("profiles").join(GAME_ID).join(PROFILE_FILE))
}

fn temporary_path(final_path: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = final_path.file_name().unwrap_or_default().to_string_lossy();
    final_path.with_file_name(format!(".{name}.tmp-{}-{sequence}", std::process::id()))
}

fn write_and_replace(
    temporary: &Path,
    final_path: &Path,
    parent: &Path,
    bytes: &[u8],
) -> Result<(), ProfileDiagnostic> {
    let mut file =
        create_private_file(temporary).map_err(|error| ProfileDiagnostic::ProfileWriteFailed {
            stage: WriteStage::CreateTemporary,
            kind: error.kind(),
        })?;
    file.write_all(bytes)
        .map_err(|error| ProfileDiagnostic::ProfileWriteFailed {
            stage: WriteStage::WriteTemporary,
            kind: error.kind(),
        })?;
    file.sync_all()
        .map_err(|error| ProfileDiagnostic::ProfileWriteFailed {
            stage: WriteStage::FlushTemporary,
            kind: error.kind(),
        })?;
    drop(file);
    atomic_replace(temporary, final_path).map_err(|error| {
        ProfileDiagnostic::ProfileWriteFailed {
            stage: WriteStage::AtomicReplace,
            kind: error.kind(),
        }
    })?;
    sync_directory(parent).map_err(|error| ProfileDiagnostic::ProfileWriteFailed {
        stage: WriteStage::SyncDirectory,
        kind: error.kind(),
    })
}

#[cfg(unix)]
fn create_private_directories(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_directories(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers that remain
    // alive for the duration of the call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}

#[derive(Debug, Default)]
struct PendingState {
    latest: Option<CalibrationSnapshot>,
    shutdown: bool,
}

pub struct PersistenceWorker {
    state: Arc<(Mutex<PendingState>, Condvar)>,
    diagnostics: mpsc::Receiver<ProfileDiagnostic>,
    stopped: mpsc::Receiver<()>,
    handle: Option<JoinHandle<()>>,
}

impl PersistenceWorker {
    /// # Errors
    ///
    /// Returns an error if the operating system cannot create the single writer thread.
    pub fn spawn(store: ProfileStore) -> io::Result<Self> {
        let state = Arc::new((Mutex::new(PendingState::default()), Condvar::new()));
        let worker_state = Arc::clone(&state);
        let (diagnostic_sender, diagnostics) = mpsc::sync_channel(8);
        let (stopped_sender, stopped) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("adaptive-eta-profile".to_owned())
            .spawn(move || {
                writer_loop(&store, &worker_state, &diagnostic_sender);
                let _ = stopped_sender.send(());
            })?;
        Ok(Self {
            state,
            diagnostics,
            stopped,
            handle: Some(handle),
        })
    }

    /// Replaces any queued state with the newest snapshot without performing I/O.
    pub fn request_save(&self, snapshot: CalibrationSnapshot) {
        let (lock, wake) = &*self.state;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.shutdown {
            state.latest = Some(snapshot);
            wake.notify_one();
        }
    }

    pub fn diagnostics(&self) -> impl Iterator<Item = ProfileDiagnostic> + '_ {
        self.diagnostics.try_iter()
    }

    /// Requests a final drain and waits no longer than `timeout` for the writer.
    /// Returns false if a filesystem operation did not finish within the bound.
    #[must_use]
    pub fn shutdown(mut self, timeout: Duration) -> bool {
        let (lock, wake) = &*self.state;
        {
            let mut state = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.shutdown = true;
            wake.notify_one();
        }
        if self.stopped.recv_timeout(timeout).is_err() {
            return false;
        }
        self.handle
            .take()
            .is_some_and(|handle| handle.join().is_ok())
    }
}

fn writer_loop(
    store: &ProfileStore,
    shared: &(Mutex<PendingState>, Condvar),
    diagnostics: &mpsc::SyncSender<ProfileDiagnostic>,
) {
    loop {
        let snapshot = {
            let (lock, wake) = shared;
            let mut state = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while state.latest.is_none() && !state.shutdown {
                state = wake
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if state.latest.is_none() && state.shutdown {
                return;
            }
            state.latest.take()
        };
        if let Some(snapshot) = snapshot {
            let diagnostic = store
                .save(snapshot)
                .map_or_else(std::convert::identity, |()| ProfileDiagnostic::ProfileSaved);
            let _ = diagnostics.try_send(diagnostic);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn learned_snapshot() -> CalibrationSnapshot {
        CalibrationSnapshot {
            model_version: CALIBRATION_MODEL_VERSION,
            log_factor: 1.2_f64.ln(),
            evidence_distance_km: 8.25,
            sample_count: 1,
        }
    }

    #[test]
    fn platform_paths_use_per_user_game_namespace() {
        let paths = PathEnvironment {
            home: Some(PathBuf::from("/Users/tester")),
            xdg_data_home: Some(PathBuf::from("/xdg/data")),
            local_app_data: Some(PathBuf::from(r"C:\Users\tester\AppData\Local")),
        };
        assert_eq!(
            resolve_profile_path(Platform::MacOs, &paths).unwrap(),
            PathBuf::from(
                "/Users/tester/Library/Application Support/Adaptive ETA/profiles/ets2/default.json"
            )
        );
        assert_eq!(
            resolve_profile_path(Platform::Linux, &paths).unwrap(),
            PathBuf::from("/xdg/data/adaptive-eta/profiles/ets2/default.json")
        );
        assert_eq!(
            resolve_profile_path(Platform::Windows, &paths).unwrap(),
            PathBuf::from(r"C:\Users\tester\AppData\Local")
                .join("Adaptive ETA/profiles/ets2/default.json")
        );
        let fallback = PathEnvironment {
            xdg_data_home: None,
            ..paths
        };
        assert_eq!(
            resolve_profile_path(Platform::Linux, &fallback).unwrap(),
            PathBuf::from("/Users/tester/.local/share/adaptive-eta/profiles/ets2/default.json")
        );
    }

    #[test]
    fn save_load_round_trip_is_exact_and_private() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles/ets2/default.json");
        let store = ProfileStore::new(path.clone());
        store.save(learned_snapshot()).unwrap();
        assert_eq!(
            store.load(),
            LoadOutcome {
                state: ProfileState::Loaded(learned_snapshot()),
                diagnostic: ProfileDiagnostic::ProfileLoaded,
                writes_allowed: true,
            }
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(directory.path().join("profiles/ets2"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn interrupted_temporary_write_leaves_final_valid() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("default.json");
        let store = ProfileStore::new(path.clone());
        store.save(learned_snapshot()).unwrap();
        fs::write(temporary_path(&path), b"{ truncated").unwrap();
        assert!(
            matches!(store.load().state, ProfileState::Loaded(snapshot) if snapshot == learned_snapshot())
        );
    }

    #[test]
    fn invalid_and_future_profiles_are_preserved_from_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("default.json");
        let store = ProfileStore::new(path.clone());
        fs::write(&path, b"{ truncated").unwrap();
        let invalid = store.load();
        assert_eq!(invalid.state, ProfileState::Degraded);
        assert!(!invalid.writes_allowed);
        assert_eq!(fs::read(&path).unwrap(), b"{ truncated");

        fs::write(
            &path,
            br#"{"schemaVersion":2,"product":"adaptive-eta","game":"ets2","calibration":{"modelVersion":1,"logFactor":0.0,"evidenceDistanceKm":0.0,"sampleCount":0}}"#,
        )
        .unwrap();
        let future = store.load();
        assert_eq!(
            future.diagnostic,
            ProfileDiagnostic::UnsupportedSchemaVersion { found: 2 }
        );
        assert!(!future.writes_allowed);
    }

    #[test]
    fn invalid_calibration_never_reaches_core() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("default.json");
        let store = ProfileStore::new(path.clone());
        for calibration in [
            r#"{"modelVersion":1,"logFactor":0.0,"evidenceDistanceKm":-1.0,"sampleCount":1}"#,
            r#"{"modelVersion":1,"logFactor":0.0,"evidenceDistanceKm":1.0,"sampleCount":0}"#,
            r#"{"modelVersion":1,"logFactor":0.0,"sampleCount":1}"#,
            r#"{"modelVersion":1,"logFactor":1e999,"evidenceDistanceKm":1.0,"sampleCount":1}"#,
        ] {
            let body = format!(
                r#"{{"schemaVersion":1,"product":"adaptive-eta","game":"ets2","calibration":{calibration}}}"#
            );
            fs::write(&path, body).unwrap();
            let outcome = store.load();
            assert_eq!(outcome.state, ProfileState::Degraded);
            assert!(!outcome.writes_allowed);
        }

        fs::write(
            &path,
            br#"{"schemaVersion":1,"product":"adaptive-eta","game":"ets2","calibration":{"modelVersion":2}}"#,
        )
        .unwrap();
        assert_eq!(
            store.load().diagnostic,
            ProfileDiagnostic::UnsupportedModelVersion { found: 2 }
        );
    }

    #[test]
    fn missing_is_fresh_and_write_failure_is_typed() {
        let directory = tempfile::tempdir().unwrap();
        let missing = ProfileStore::new(directory.path().join("missing/default.json"));
        assert_eq!(missing.load().state, ProfileState::Fresh);

        let unreadable_as_profile = ProfileStore::new(directory.path().to_path_buf()).load();
        assert!(matches!(
            unreadable_as_profile.diagnostic,
            ProfileDiagnostic::ProfileReadFailed { .. }
        ));
        assert!(!unreadable_as_profile.writes_allowed);

        let blocker = directory.path().join("not-a-directory");
        fs::write(&blocker, b"blocker").unwrap();
        let failing = ProfileStore::new(blocker.join("default.json"));
        assert!(matches!(
            failing.save(learned_snapshot()),
            Err(ProfileDiagnostic::ProfileWriteFailed {
                stage: WriteStage::CreateDirectory,
                ..
            })
        ));
    }

    #[test]
    fn worker_coalesces_and_flushes_latest_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(directory.path().join("default.json"));
        let worker = PersistenceWorker::spawn(store.clone()).unwrap();
        worker.request_save(CalibrationSnapshot {
            evidence_distance_km: 16.5,
            sample_count: 2,
            ..learned_snapshot()
        });
        let latest = CalibrationSnapshot {
            evidence_distance_km: 24.75,
            sample_count: 3,
            ..learned_snapshot()
        };
        worker.request_save(latest);
        assert!(worker.shutdown(Duration::from_secs(5)));
        assert!(matches!(store.load().state, ProfileState::Loaded(snapshot) if snapshot == latest));
    }
}
