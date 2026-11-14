//! One Janis at a time.
//!
//! Two processes would race the write lock on `janis.db`, so a second launch
//! hands over to the running one — which comes to the front — and exits.
//!
//! The running instance listens on a loopback port of the system's choosing
//! and leaves its number in a file beside the database; a launch that
//! reaches it there says hello, hears the answer and leaves. A stale file —
//! the instance that wrote it is gone — fails to connect, or reaches some
//! other program that does not answer as Janis would, and the launch carries
//! on as the one instance.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PORT_FILE: &str = "instance.port";
/// What a second launch says, so that a stray connection is not mistaken for one.
const GREETING: &[u8] = b"janis:show";
/// What the running instance answers, so that another program listening on a
/// port a stale file names is not mistaken for it.
const ANSWER: &[u8] = b"janis:here";
const PATIENCE: Duration = Duration::from_millis(500);

/// Held by the one running instance for as long as it runs.
pub struct Instance {
    port_file: PathBuf,
    port: u16,
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Only this instance's own file: one written since by another
        // instance is that one's to remove.
        if recorded_port(&self.port_file) == Some(self.port) {
            let _ = std::fs::remove_file(&self.port_file);
        }
    }
}

fn recorded_port(port_file: &Path) -> Option<u16> {
    std::fs::read_to_string(port_file)
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

/// Claims the instance, calling `shown` — from a thread of its own — whenever
/// a later launch asks this one to come forward. Returns `None` when another
/// instance is already running and has been asked to; the caller should exit.
pub fn claim(
    data_directory: &Path,
    shown: impl Fn() + Send + 'static,
) -> Result<Option<Instance>, String> {
    let port_file = data_directory.join(PORT_FILE);
    if greet_running_instance(&port_file) {
        return Ok(None);
    }

    std::fs::create_dir_all(data_directory)
        .map_err(|error| format!("create {}: {error}", data_directory.display()))?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("listen for later launches: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("read the instance port: {error}"))?
        .port();
    std::fs::write(&port_file, port.to_string())
        .map_err(|error| format!("write {}: {error}", port_file.display()))?;

    std::thread::Builder::new()
        .name("janis instance".to_owned())
        .spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut greeting = [0u8; GREETING.len()];
                let _ = stream.set_read_timeout(Some(PATIENCE));
                if stream.read_exact(&mut greeting).is_ok() && greeting == GREETING {
                    let _ = stream.write_all(ANSWER);
                    shown();
                }
            }
        })
        .map_err(|error| format!("start the instance thread: {error}"))?;

    Ok(Some(Instance { port_file, port }))
}

/// True when a running instance was reached, asked to come forward, and
/// answered.
fn greet_running_instance(port_file: &Path) -> bool {
    let Some(port) = recorded_port(port_file) else {
        return false;
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, PATIENCE) else {
        return false;
    };
    let mut answer = [0u8; ANSWER.len()];
    stream.set_read_timeout(Some(PATIENCE)).is_ok()
        && stream.write_all(GREETING).is_ok()
        && stream.read_exact(&mut answer).is_ok()
        && answer == ANSWER
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn a_second_launch_brings_the_first_forward_and_steps_aside() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (shown, was_shown) = channel();
        let first = claim(directory.path(), move || {
            let _ = shown.send(());
        })
        .expect("the first launch claims the instance");
        assert!(first.is_some());

        let second = claim(directory.path(), || {}).expect("the second launch finds the first");

        assert!(second.is_none());
        was_shown
            .recv_timeout(Duration::from_secs(5))
            .expect("the first instance is asked to come forward");
    }

    #[test]
    fn another_program_on_the_recorded_port_does_not_block_a_launch() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        // Something else listens where a stale file points, and never
        // answers as Janis would.
        let stranger = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a free port");
        let port = stranger.local_addr().expect("its address").port();
        std::fs::write(directory.path().join(PORT_FILE), port.to_string()).expect("write");

        let launch = claim(directory.path(), || {}).expect("the launch carries on");

        assert!(launch.is_some());
        drop(launch);
        drop(stranger);
    }

    #[test]
    fn an_instance_leaves_the_port_file_of_another_alone() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let first = claim(directory.path(), || {}).expect("the launch claims the instance");
        // Another instance has since recorded its own port.
        std::fs::write(directory.path().join(PORT_FILE), "1").expect("write");

        drop(first);

        assert!(directory.path().join(PORT_FILE).exists());
    }

    #[test]
    fn a_stale_port_file_does_not_block_a_launch() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        // A port nothing listens on any more: bind one, then let it go.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        std::fs::write(directory.path().join(PORT_FILE), port.to_string()).expect("write");

        let launch = claim(directory.path(), || {}).expect("the launch carries on");

        assert!(launch.is_some());
        drop(launch);
        assert!(
            !directory.path().join(PORT_FILE).exists(),
            "the instance takes its port file with it"
        );
    }
}
