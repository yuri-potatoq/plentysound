use crate::app::DaemonApp;
use crate::protocol::{socket_path, ClientCommand, DaemonEvent, recv_message, send_message};
use anyhow::{Context, Result};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub fn run_daemon() -> Result<()> {
    let sock_path = socket_path();

    // Check for stale socket
    if sock_path.exists() {
        match UnixStream::connect(&sock_path) {
            Ok(_) => {
                anyhow::bail!(
                    "Another daemon is already running (socket {} is active)",
                    sock_path.display()
                );
            }
            Err(_) => {
                let _ = std::fs::remove_file(&sock_path);
            }
        }
    }

    let listener = UnixListener::bind(&sock_path)
        .with_context(|| format!("Failed to bind socket at {}", sock_path.display()))?;
    listener.set_nonblocking(true)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    setup_signal_handler(shutdown.clone());

    let mut app = DaemonApp::new();

    // Broadcast channels: each client writer thread gets a receiver
    let client_senders: Arc<Mutex<Vec<mpsc::Sender<DaemonEvent>>>> =
        Arc::new(Mutex::new(Vec::new()));

    // Channel for client commands forwarded to daemon main loop
    let (cmd_tx, cmd_rx) = mpsc::channel::<ClientCommand>();

    // Tray state
    let tray_now_playing: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    crate::tray::spawn_tray(shutdown.clone(), tray_now_playing.clone());

    eprintln!(
        "plentysound daemon started (socket: {})",
        sock_path.display()
    );

    loop {
        // Accept new connections
        match listener.accept() {
            Ok((stream, _)) => {
                handle_new_client(stream, &app, &cmd_tx, &client_senders);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                crate::log::log_error(&format!("Accept error: {e}"));
            }
        }

        // Process commands from clients
        while let Ok(cmd) = cmd_rx.try_recv() {
            let events = app.apply_command(cmd);
            for event in &events {
                if matches!(event, DaemonEvent::Shutdown) {
                    shutdown.store(true, Ordering::SeqCst);
                }
                update_tray_np(&tray_now_playing, event);
            }
            broadcast(&client_senders, &events);
        }

        // Process PipeWire events
        let pw_events = app.process_pw_events();
        if !pw_events.is_empty() {
            for event in &pw_events {
                update_tray_np(&tray_now_playing, event);
            }
            broadcast(&client_senders, &pw_events);
        }

        if shutdown.load(Ordering::SeqCst) {
            broadcast(&client_senders, &[DaemonEvent::Shutdown]);
            break;
        }

        std::thread::sleep(Duration::from_millis(20));
    }

    let _ = std::fs::remove_file(&sock_path);
    eprintln!("plentysound daemon stopped.");
    // Force exit: tray thread (ksni D-Bus loop) and PipeWire playback threads
    // may keep the process alive otherwise.
    std::process::exit(0);
}

fn handle_new_client(
    stream: UnixStream,
    app: &DaemonApp,
    cmd_tx: &mpsc::Sender<ClientCommand>,
    client_senders: &Arc<Mutex<Vec<mpsc::Sender<DaemonEvent>>>>,
) {
    let snapshot = app.snapshot();
    let (event_tx, event_rx) = mpsc::channel::<DaemonEvent>();

    let mut write_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };

    // Send initial state
    if send_message(&mut write_stream, &DaemonEvent::State(snapshot)).is_err() {
        return;
    }

    client_senders.lock().unwrap().push(event_tx);

    // Reader thread
    let read_cmd_tx = cmd_tx.clone();
    std::thread::spawn(move || {
        let mut read_stream = stream;
        read_stream.set_nonblocking(false).ok();
        loop {
            match recv_message::<ClientCommand>(&mut read_stream) {
                Ok(cmd) => {
                    if read_cmd_tx.send(cmd).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Writer thread
    std::thread::spawn(move || {
        for event in event_rx {
            let is_shutdown = matches!(event, DaemonEvent::Shutdown);
            if send_message(&mut write_stream, &event).is_err() {
                break;
            }
            if is_shutdown {
                break;
            }
        }
    });
}

fn broadcast(client_senders: &Arc<Mutex<Vec<mpsc::Sender<DaemonEvent>>>>, events: &[DaemonEvent]) {
    let mut senders = client_senders.lock().unwrap();
    for event in events {
        senders.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

fn update_tray_np(tray_np: &Arc<Mutex<Option<String>>>, event: &DaemonEvent) {
    match event {
        DaemonEvent::NowPlaying(np) => {
            *tray_np.lock().unwrap() = np.clone();
        }
        DaemonEvent::State(state) => {
            *tray_np.lock().unwrap() = state.now_playing.clone();
        }
        DaemonEvent::PlaybackFinished => {
            *tray_np.lock().unwrap() = None;
        }
        _ => {}
    }
}

static SIGNAL_PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

fn setup_signal_handler(shutdown: Arc<AtomicBool>) {
    let mut fds = [0i32; 2];
    unsafe {
        libc::pipe(fds.as_mut_ptr());
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    SIGNAL_PIPE_WRITE.store(write_fd, Ordering::SeqCst);

    unsafe {
        libc::signal(libc::SIGINT, signal_handler as usize);
        libc::signal(libc::SIGTERM, signal_handler as usize);
    }

    std::thread::spawn(move || {
        let mut buf = [0u8; 1];
        unsafe {
            libc::read(read_fd, buf.as_mut_ptr() as *mut _, 1);
        }
        shutdown.store(true, Ordering::SeqCst);
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
    });
}

extern "C" fn signal_handler(_sig: i32) {
    let fd = SIGNAL_PIPE_WRITE.load(Ordering::SeqCst);
    if fd >= 0 {
        unsafe {
            libc::write(fd, b"x".as_ptr() as *const _, 1);
        }
    }
}
