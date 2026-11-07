// Opens the speech door and serves it until a minute passes with nobody connected.
// SPDX-License-Identifier: GPL-3.0-or-later
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::door::{Speaker, serve};
use super::play::{Cues, Player};
use super::queue::Engine;

const IDLE: Duration = Duration::from_secs(60);
const BUSY_CHECK: Duration = Duration::from_secs(1);

pub fn door_path() -> Result<PathBuf, String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()).ok_or("XDG_RUNTIME_DIR is not set to a full path")?;
    let (was, me) = (std::fs::metadata(&dir).map_err(|e| e.to_string())?, std::fs::metadata("/proc/self").map_err(|e| e.to_string())?);
    if !was.is_dir() || was.uid() != me.uid() || was.mode() & 0o077 != 0 {
        return Err("XDG_RUNTIME_DIR is not a folder for this person alone".into());
    }
    Ok(dir.join("trindle-voice").join("say.sock"))
}

fn open_door(path: &Path) -> Result<UnixListener, String> {
    let folder = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    if path.exists() {

        match UnixStream::connect(path) {
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(path);
            }
            _ => return Err(format!("{}: another speech server is already running", path.display())),
        }
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    Ok(listener)
}

fn forget(path: &Path, inode: u64) {
    if std::fs::metadata(path).is_ok_and(|m| m.ino() == inode) {
        let _ = std::fs::remove_file(path);
    }
}

fn forget_ours(path: &Path, inode: Option<u64>) {
    if let Some(inode) = inode {
        forget(path, inode);
    }
}

fn inherited() -> Option<UnixListener> {
    from_systemd(std::env::var("LISTEN_PID").ok().as_deref(), std::env::var("LISTEN_FDS").ok().as_deref(), std::process::id(), 3)
}

fn from_systemd(pid: Option<&str>, fds: Option<&str>, me: u32, fd: RawFd) -> Option<UnixListener> {
    if pid?.parse::<u32>().ok()? != me || fds?.parse::<u32>().ok()? != 1 {
        return None;
    }
    let kind = std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()?;
    if !kind.to_str()?.starts_with("socket:") {
        return None;
    }

    let listener = unsafe { UnixListener::from_raw_fd(fd) };
    listener.local_addr().ok()?;
    Some(listener)
}

pub fn run(folder: &Path, path: &Path) -> Result<(), String> {
    run_checked(folder, path, super::reader::unchecked)
}

pub fn run_checked(folder: &Path, path: &Path, check: super::reader::Check) -> Result<(), String> {
    let (listener, inode) = match inherited() {
        Some(listener) => (listener, None),
        None => {
            let listener = open_door(path)?;
            (listener, Some(std::fs::metadata(path).map_err(|e| e.to_string())?.ino()))
        }
    };
    let engine = Arc::new(Engine::start(Arc::new(Speaker::checked(folder.to_path_buf(), check)), Box::new(Player::new())));
    let cues = Arc::new(Mutex::new(Cues::default()));
    let busy = Arc::new(AtomicUsize::new(0));
    let (knock, knocks) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            if knock.send(stream).is_err() {
                return;
            }
        }
    });
    let mut last = Instant::now();
    loop {
        let wait = if busy.load(Ordering::SeqCst) > 0 { BUSY_CHECK } else { IDLE.saturating_sub(last.elapsed()) };
        let mut knocked = false;
        match knocks.recv_timeout(wait) {
            Ok(stream) => {
                knocked = true;
                let (engine, cues, busy, folder) = (engine.clone(), cues.clone(), busy.clone(), folder.to_path_buf());
                busy.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    serve(stream, engine, &folder, cues);
                    busy.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(RecvTimeoutError::Disconnected) => {
                forget_ours(path, inode);
                return Err("the door stopped listening".into());
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if knocked || busy.load(Ordering::SeqCst) > 0 {
            last = Instant::now();
        } else if last.elapsed() > IDLE {
            forget_ours(path, inode);
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::IntoRawFd;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tv-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_unix_socket_for_this_process_is_taken() {
        let dir = temp("sock");
        let fd = UnixListener::bind(dir.join("s")).unwrap().into_raw_fd();
        let me = std::process::id();
        assert!(from_systemd(Some(&me.to_string()), Some("1"), me, fd).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn another_process_or_count_or_nothing_is_ignored() {
        let dir = temp("other");
        let fd = std::fs::File::create(dir.join("f")).unwrap().into_raw_fd();
        let me = std::process::id();
        assert!(from_systemd(Some(&(me + 1).to_string()), Some("1"), me, fd).is_none());
        assert!(from_systemd(Some(&me.to_string()), Some("2"), me, fd).is_none());
        assert!(from_systemd(None, Some("1"), me, fd).is_none());
        assert!(from_systemd(Some(&me.to_string()), None, me, fd).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_descriptor_that_is_not_a_socket_is_left_alone() {
        let dir = temp("file");
        let fd = std::fs::File::create(dir.join("f")).unwrap().into_raw_fd();
        let me = std::process::id();
        assert!(from_systemd(Some(&me.to_string()), Some("1"), me, fd).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
