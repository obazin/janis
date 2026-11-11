//! One Janis at a time.
//!
//! Two processes would race the write lock on `janis.db`, so a second launch
//! hands over to the running one — which comes to the front — and exits.
//!
//! The running instance listens on a loopback port of the system's choosing
//! and leaves its number in a file beside the database; a launch that can
//! reach it there says hello and leaves. A stale file — the instance that
//! wrote it is gone — simply fails to connect, and the launch carries on as
//! the one instance.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PORT_FILE: &str = "instance.port";
/// What a second launch says, so that a stray connection is not mistaken for one.
const GREETING: &[u8] = b"janis:show";
const PATIENCE: Duration = Duration::from_millis(500);

/// Held by the one running instance for as long as it runs.
pub struct Instance {
    port_file: PathBuf,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.port_file);
    }
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
                    shown();
                }
            }
        })
        .map_err(|error| format!("start the instance thread: {error}"))?;

    Ok(Some(Instance { port_file }))
}

/// True when a running instance was reached and asked to come forward.
fn greet_running_instance(port_file: &Path) -> bool {
    let Some(port) = std::fs::read_to_string(port_file)
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
    else {
        return false;
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, PATIENCE)
        .and_then(|mut stream| stream.write_all(GREETING))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("janis-instance-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn a_second_launch_brings_the_first_forward_and_steps_aside() {
        let directory = directory("second");
        let (shown, was_shown) = channel();
        let first = claim(&directory, move || {
            let _ = shown.send(());
        })
        .expect("the first launch claims the instance");
        assert!(first.is_some());

        let second = claim(&directory, || {}).expect("the second launch finds the first");

        assert!(second.is_none());
        was_shown
            .recv_timeout(Duration::from_secs(5))
            .expect("the first instance is asked to come forward");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_stale_port_file_does_not_block_a_launch() {
        let directory = directory("stale");
        std::fs::create_dir_all(&directory).expect("temp dir");
        // A port nothing listens on any more: bind one, then let it go.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        std::fs::write(directory.join(PORT_FILE), port.to_string()).expect("write");

        let launch = claim(&directory, || {}).expect("the launch carries on");

        assert!(launch.is_some());
        drop(launch);
        assert!(
            !directory.join(PORT_FILE).exists(),
            "the instance takes its port file with it"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
