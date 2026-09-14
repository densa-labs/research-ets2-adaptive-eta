use std::fs::{self, DirBuilder, FileType};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const DIRECTORY_PREFIX: &str = "adaptive-eta-";
const SOCKET_NAME: &str = "telemetry-v1.sock";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    directory: PathBuf,
    socket: PathBuf,
    uid: u32,
}

#[derive(Debug)]
pub enum EndpointError {
    HomeUnavailable,
    UnsafeDirectory { path: PathBuf, detail: &'static str },
    UnsafeSocket { path: PathBuf, detail: &'static str },
    EndpointInUse(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable => formatter.write_str("HOME is unavailable"),
            Self::UnsafeDirectory { path, detail } => {
                write!(
                    formatter,
                    "unsafe runtime directory {}: {detail}",
                    path.display()
                )
            }
            Self::UnsafeSocket { path, detail } => {
                write!(formatter, "unsafe socket {}: {detail}", path.display())
            }
            Self::EndpointInUse(path) => {
                write!(formatter, "endpoint is already active: {}", path.display())
            }
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for EndpointError {}

impl From<io::Error> for EndpointError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl Endpoint {
    /// Derives a short per-user endpoint from the owner of the user's home.
    ///
    /// # Errors
    ///
    /// Returns an error if `HOME` is unavailable or cannot be inspected.
    pub fn for_current_user() -> Result<Self, EndpointError> {
        let home = std::env::var_os("HOME").ok_or(EndpointError::HomeUnavailable)?;
        let uid = fs::metadata(home)?.uid();
        #[cfg(target_os = "linux")]
        if let Some(runtime_directory) = std::env::var_os("XDG_RUNTIME_DIR") {
            return Ok(Self::for_uid_in(uid, Path::new(&runtime_directory)));
        }
        Ok(Self::for_uid_in(uid, Path::new("/tmp")))
    }

    #[must_use]
    pub fn for_uid_in(uid: u32, base: &Path) -> Self {
        let directory = base.join(format!("{DIRECTORY_PREFIX}{uid}"));
        let socket = directory.join(SOCKET_NAME);
        Self {
            directory,
            socket,
            uid,
        }
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    #[must_use]
    pub fn directory_path(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn prepare_for_bind(&self) -> Result<(), EndpointError> {
        match fs::symlink_metadata(&self.directory) {
            Ok(metadata) => self.validate_directory(&metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                DirBuilder::new().mode(0o700).create(&self.directory)?;
                let metadata = fs::symlink_metadata(&self.directory)?;
                self.validate_directory(&metadata)?;
            }
            Err(error) => return Err(error.into()),
        }
        self.remove_stale_socket()
    }

    pub(crate) fn secure_bound_socket(&self) -> Result<(), EndpointError> {
        fs::set_permissions(&self.socket, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&self.socket)?;
        self.validate_socket(&metadata)
    }

    pub(crate) fn socket_identity(&self) -> Result<(u64, u64), EndpointError> {
        let metadata = fs::symlink_metadata(&self.socket)?;
        self.validate_socket(&metadata)?;
        Ok((metadata.dev(), metadata.ino()))
    }

    pub(crate) fn cleanup_owned_socket(&self, expected_identity: Option<(u64, u64)>) {
        let Ok(metadata) = fs::symlink_metadata(&self.socket) else {
            return;
        };
        let identity_matches =
            expected_identity.is_none_or(|identity| identity == (metadata.dev(), metadata.ino()));
        if identity_matches && self.validate_socket(&metadata).is_ok() {
            let _ = fs::remove_file(&self.socket);
        }
    }

    fn validate_directory(&self, metadata: &fs::Metadata) -> Result<(), EndpointError> {
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(self.unsafe_directory("not a real directory"));
        }
        if metadata.uid() != self.uid {
            return Err(self.unsafe_directory("owned by another user"));
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(self.unsafe_directory("permissions must be 0700 or stricter"));
        }
        Ok(())
    }

    fn validate_socket(&self, metadata: &fs::Metadata) -> Result<(), EndpointError> {
        if !is_socket(metadata.file_type()) || metadata.file_type().is_symlink() {
            return Err(self.unsafe_socket("not a Unix socket"));
        }
        if metadata.uid() != self.uid {
            return Err(self.unsafe_socket("owned by another user"));
        }
        Ok(())
    }

    fn remove_stale_socket(&self) -> Result<(), EndpointError> {
        let metadata = match fs::symlink_metadata(&self.socket) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        self.validate_socket(&metadata)?;
        match std::os::unix::net::UnixDatagram::unbound()?.connect(&self.socket) {
            Ok(()) => Err(EndpointError::EndpointInUse(self.socket.clone())),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) =>
            {
                fs::remove_file(&self.socket)?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn unsafe_directory(&self, detail: &'static str) -> EndpointError {
        EndpointError::UnsafeDirectory {
            path: self.directory.clone(),
            detail,
        }
    }

    fn unsafe_socket(&self, detail: &'static str) -> EndpointError {
        EndpointError::UnsafeSocket {
            path: self.socket.clone(),
            detail,
        }
    }
}

fn is_socket(file_type: FileType) -> bool {
    file_type.is_socket()
}
