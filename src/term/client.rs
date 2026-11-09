#![cfg(unix)]

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::term::TermError;
use crate::term::proto::{Request, RequestOp, Response, read_response, write_request};

const MAX_UNIX_SOCKET_PATH_BYTES: usize = 107;
const HOST_START_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_START_POLL: Duration = Duration::from_millis(20);

pub struct Client {
    stream: UnixStream,
}

impl Client {
    pub fn request(&mut self, op: RequestOp) -> Result<Response, TermError> {
        write_request(&mut self.stream, &Request::new(op)).map_err(|error| {
            TermError::internal(format!("could not write to the term host: {error}"))
        })?;
        read_response(&mut self.stream).map_err(|error| {
            TermError::internal(format!("could not read from the term host: {error}"))
        })
    }

    pub fn call(&mut self, op: RequestOp) -> Result<Response, TermError> {
        let response = self.request(op)?;
        if response.ok {
            Ok(response)
        } else {
            Err(TermError::new(
                response.exit_code.unwrap_or(crate::term::exit::INTERNAL),
                response
                    .error
                    .unwrap_or_else(|| "term host request failed".to_owned()),
            ))
        }
    }
}

pub fn try_connect(workspace_root: &Path) -> io::Result<Client> {
    let path = socket_path(workspace_root)?;
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    Ok(Client { stream })
}

pub fn connect(workspace_root: &Path) -> Result<Client, TermError> {
    if let Ok(client) = try_connect(workspace_root) {
        return Ok(client);
    }
    spawn_host(workspace_root)?;
    let started = Instant::now();
    loop {
        match try_connect(workspace_root) {
            Ok(client) => return Ok(client),
            Err(error) => {
                if started.elapsed() >= HOST_START_TIMEOUT {
                    return Err(TermError::internal(format!(
                        "the terminal session host did not start: {error}"
                    )));
                }
            }
        }
        thread::sleep(HOST_START_POLL);
    }
}

pub fn spawn_host(workspace_root: &Path) -> Result<(), TermError> {
    let mut command = host_command(workspace_root);
    detach(&mut command);
    let child = command.spawn().map_err(|error| {
        TermError::internal(format!(
            "could not start the terminal session host: {error}"
        ))
    })?;
    drop(child);
    Ok(())
}

pub fn socket_path(workspace_root: &Path) -> io::Result<PathBuf> {
    let key = crate::state_path::repo_key(workspace_root)?;
    let preferred = crate::state_path::state_base()?
        .join("repos")
        .join(&key)
        .join("term.sock");
    let fallback = fallback_socket_path(&key);
    Ok(choose_socket_path(preferred, fallback))
}

fn fallback_socket_path(repo_key: &str) -> PathBuf {
    let uid = unsafe { libc::geteuid() };
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("dmuxai-{uid}"))
        .join(repo_key)
        .join("term.sock")
}

fn choose_socket_path(preferred: PathBuf, fallback: PathBuf) -> PathBuf {
    if socket_path_fits(&preferred) {
        preferred
    } else {
        fallback
    }
}

fn socket_path_fits(path: &Path) -> bool {
    path.as_os_str().as_bytes().len() <= MAX_UNIX_SOCKET_PATH_BYTES
}

fn host_command(workspace_root: &Path) -> Command {
    let executable = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dmuxai"));
    let mut command = Command::new(executable);
    command
        .arg("-C")
        .arg(workspace_root)
        .arg("term")
        .arg("__host")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc_2601_long_socket_path_uses_the_short_fallback() {
        let long = PathBuf::from(format!("/tmp/{}", "a".repeat(200))).join("term.sock");
        let short = PathBuf::from("/tmp/dmuxai-1/repo/term.sock");
        assert!(!socket_path_fits(&long));
        assert!(socket_path_fits(&short));
        assert_eq!(choose_socket_path(long, short.clone()), short);
        assert_eq!(
            choose_socket_path(
                PathBuf::from("/tmp/short/term.sock"),
                PathBuf::from("/tmp/other/term.sock")
            ),
            PathBuf::from("/tmp/short/term.sock")
        );
    }
}
