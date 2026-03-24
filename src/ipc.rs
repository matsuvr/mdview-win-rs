use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    os::windows::ffi::{OsStrExt, OsStringExt},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process,
    sync::mpsc::Sender,
    thread,
    time::{Duration, Instant},
};

use interprocess::local_socket::{
    GenericNamespaced, Listener, ListenerOptions, Stream, ToNsName,
    traits::{Listener as _, Stream as _},
};

const SOCKET_NAME: &str = "mdview.gpui.windows.x64";
const CONNECT_RETRY_WINDOW: Duration = Duration::from_secs(2);
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(25);

pub enum IpcMode {
    Primary(PrimaryEndpoint),
    Secondary(Stream),
}

pub struct PrimaryEndpoint {
    listener: Listener,
    instance_lock: PrimaryInstanceLock,
}

impl PrimaryEndpoint {
    pub fn into_parts(self) -> (Listener, PrimaryInstanceLock) {
        (self.listener, self.instance_lock)
    }
}

pub struct PrimaryInstanceLock {
    path: PathBuf,
    file: Option<File>,
}

impl PrimaryInstanceLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(path)?;

        writeln!(file, "{}", process::id())?;
        file.flush()?;

        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
        })
    }
}

impl Drop for PrimaryInstanceLock {
    fn drop(&mut self) {
        drop(self.file.take());

        let remove_result = fs::remove_file(&self.path);

        #[cfg(debug_assertions)]
        if let Err(error) = &remove_result {
            if error.kind() != ErrorKind::NotFound {
                eprintln!(
                    "failed to remove instance lock {}: {error}",
                    self.path.display()
                );
            }
        }

        #[cfg(not(debug_assertions))]
        let _ = remove_result;
    }
}

pub fn try_establish_endpoint() -> io::Result<IpcMode> {
    let name = SOCKET_NAME
        .to_ns_name::<GenericNamespaced>()
        .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error.to_string()))?;
    let lock_path = instance_lock_path();

    loop {
        match PrimaryInstanceLock::acquire(&lock_path) {
            Ok(instance_lock) => {
                let listener = ListenerOptions::new().name(name.clone()).create_sync()?;
                return Ok(IpcMode::Primary(PrimaryEndpoint {
                    listener,
                    instance_lock,
                }));
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                match connect_to_primary(&name) {
                    Ok(stream) => return Ok(IpcMode::Secondary(stream)),
                    Err(connect_error) => {
                        if reclaim_stale_lock(&lock_path)? {
                            continue;
                        }
                        return Err(connect_error);
                    }
                }
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn forward_to_primary(stream: &mut Stream, paths: &[PathBuf]) -> io::Result<()> {
    write_request(stream, paths)
}

pub fn spawn_listener_thread(
    listener: Listener,
    tx: Sender<Vec<PathBuf>>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("mdview-ipc-listener".into())
        .spawn(move || {
            loop {
                let mut stream = match listener.accept() {
                    Ok(stream) => stream,
                    Err(error) => {
                        eprintln!("IPC accept failed: {error}");
                        continue;
                    }
                };

                let paths = match read_request(&mut stream) {
                    Ok(paths) => paths,
                    Err(error) => {
                        eprintln!("IPC read failed: {error}");
                        continue;
                    }
                };

                if tx.send(paths).is_err() {
                    break;
                }
            }
        })
        .expect("failed to spawn IPC listener thread")
}

fn write_request(stream: &mut Stream, paths: &[PathBuf]) -> io::Result<()> {
    write_u32(stream, paths.len() as u32)?;
    for path in paths {
        write_path(stream, path)?;
    }
    stream.flush()
}

fn read_request(stream: &mut Stream) -> io::Result<Vec<PathBuf>> {
    const MAX_PATH_COUNT: usize = 1024;

    let count = read_u32(stream)? as usize;
    if count > MAX_PATH_COUNT {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("path count {count} exceeds the limit of {MAX_PATH_COUNT}"),
        ));
    }

    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        paths.push(read_path(stream)?);
    }
    Ok(paths)
}

fn write_path(stream: &mut Stream, path: &Path) -> io::Result<()> {
    let wide: Vec<u16> = path_to_wide(path);
    write_u32(stream, wide.len() as u32)?;
    for unit in wide {
        stream.write_all(&unit.to_le_bytes())?;
    }
    Ok(())
}

fn read_path(stream: &mut Stream) -> io::Result<PathBuf> {
    const MAX_PATH_WIDE_LEN: usize = 32768;

    let len = read_u32(stream)? as usize;
    if len > MAX_PATH_WIDE_LEN {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("path length {len} exceeds the limit of {MAX_PATH_WIDE_LEN}"),
        ));
    }

    let mut wide = Vec::with_capacity(len);
    for _ in 0..len {
        let mut bytes = [0u8; 2];
        stream.read_exact(&mut bytes)?;
        wide.push(u16::from_le_bytes(bytes));
    }
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

fn path_to_wide(path: &Path) -> Vec<u16> {
    os_str_to_wide(path.as_os_str())
}

fn os_str_to_wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().collect()
}

fn write_u32(stream: &mut Stream, value: u32) -> io::Result<()> {
    stream.write_all(&value.to_le_bytes())
}

fn read_u32(stream: &mut Stream) -> io::Result<u32> {
    let mut bytes = [0u8; 4];
    stream.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn connect_to_primary(name: &interprocess::local_socket::Name<'static>) -> io::Result<Stream> {
    let start = Instant::now();
    let mut last_error = None;

    while start.elapsed() <= CONNECT_RETRY_WINDOW {
        match Stream::connect(name.clone()) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(CONNECT_RETRY_INTERVAL);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            ErrorKind::TimedOut,
            "timed out while waiting for the primary instance",
        )
    }))
}

fn reclaim_stale_lock(lock_path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(lock_path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) if error.kind() == ErrorKind::PermissionDenied => Ok(false),
        Err(error) => Err(error),
    }
}

fn instance_lock_path() -> PathBuf {
    std::env::temp_dir().join(format!("{SOCKET_NAME}.lock"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::PrimaryInstanceLock;

    #[test]
    #[cfg(target_os = "windows")]
    fn primary_instance_lock_removes_file_on_drop() {
        let lock_path = unique_lock_path();
        let _ = fs::remove_file(&lock_path);

        let first_lock = PrimaryInstanceLock::acquire(&lock_path)
            .expect("should create the initial instance lock");
        assert!(lock_path.exists(), "lock file should exist while held");

        drop(first_lock);
        assert!(
            !lock_path.exists(),
            "lock file should be removed after dropping the lock"
        );

        let second_lock = PrimaryInstanceLock::acquire(&lock_path)
            .expect("should reacquire the lock immediately after cleanup");
        drop(second_lock);
        assert!(
            !lock_path.exists(),
            "lock file should still be removed after a second release"
        );
    }

    #[cfg(target_os = "windows")]
    fn unique_lock_path() -> PathBuf {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after the Unix epoch")
            .as_nanos();

        std::env::temp_dir().join(format!(
            "mdview-ipc-test-{}-{unique_suffix}.lock",
            process::id()
        ))
    }
}
