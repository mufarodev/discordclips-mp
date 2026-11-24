//! Unix-socket server and client for the [`proto`](crate::proto) vocabulary.

use crate::proto::{read_frame, write_frame, Command, Event};
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub trait Service: Send + Sync + 'static {
    fn handle(&self, cmd: Command) -> Event;
}

/// Bind `path` (removing a stale socket) and serve until a `Shutdown` command has been handled.
pub fn serve<S: Service>(path: &Path, service: Arc<S>) -> io::Result<()> {
    if UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AddrInUse, "another clipsd is already serving this socket"));
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    // Owner-only: the socket can start/stop recording and write files.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut conns = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((conn, _)) => {
                conn.set_nonblocking(false)?;
                let (service, stop) = (service.clone(), stop.clone());
                conns.push(std::thread::spawn(move || handle_conn(conn, service, stop)));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                let _ = std::fs::remove_file(path);
                return Err(e);
            }
        }
    }
    let _ = std::fs::remove_file(path);
    for c in conns {
        let _ = c.join();
    }
    Ok(())
}

fn handle_conn<S: Service>(mut conn: UnixStream, service: Arc<S>, stop: Arc<AtomicBool>) {
    while let Ok(Some(cmd)) = read_frame::<_, Command>(&mut conn) {
        let shutdown = matches!(cmd, Command::Shutdown);
        let ev = service.handle(cmd);
        if write_frame(&mut conn, &ev).is_err() {
            return;
        }
        if shutdown {
            stop.store(true, Ordering::Relaxed);
            return;
        }
    }
}

pub struct Client {
    conn: UnixStream,
}

impl Client {
    pub fn connect(path: &Path) -> io::Result<Self> {
        Ok(Self { conn: UnixStream::connect(path)? })
    }

    pub fn call(&mut self, cmd: &Command) -> io::Result<Event> {
        write_frame(&mut self.conn, cmd)?;
        read_frame::<_, Event>(&mut self.conn)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "server closed connection"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Echo(Mutex<Vec<Command>>);
    impl Service for Echo {
        fn handle(&self, cmd: Command) -> Event {
            self.0.lock().unwrap().push(cmd.clone());
            match cmd {
                Command::GetStats => Event::Stats(Default::default()),
                Command::Shutdown => Event::Ack,
                _ => Event::Error { message: "unsupported".into() },
            }
        }
    }

    #[test]
    fn serve_roundtrip_and_shutdown() {
        let dir = std::env::temp_dir().join(format!("clipsd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s.sock");
        let svc = Arc::new(Echo(Mutex::new(vec![])));
        let t = {
            let (sock, svc) = (sock.clone(), svc.clone());
            std::thread::spawn(move || serve(&sock, svc).unwrap())
        };
        for _ in 0..100 {
            if sock.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut c = Client::connect(&sock).unwrap();
        assert_eq!(c.call(&Command::GetStats).unwrap(), Event::Stats(Default::default()));
        // a second server on the same socket must refuse
        assert!(serve(&sock, svc.clone()).is_err());
        assert_eq!(c.call(&Command::Shutdown).unwrap(), Event::Ack);
        t.join().unwrap();
        assert!(!sock.exists(), "socket removed on shutdown");
        assert_eq!(svc.0.lock().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
