use crate::app::DaemonApp;
use crate::protocol::{socket_path, ClientCommand, DaemonEvent, recv_message, send_message};
use anyhow::{Context, Result};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(feature = "transcriber")]
use crate::protocol::WordDetectorStatus;

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

    #[cfg(feature = "transcriber")]
    let mut download_spawned = false;

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
            crate::log::log_info(&format!("Processing command: {:?}", cmd));
            let events = app.apply_command(cmd);
            crate::log::log_info(&format!("Command produced {} events, broadcasting", events.len()));
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
            #[cfg(feature = "transcriber")]
            let mut autostarted = false;
            #[cfg(feature = "transcriber")]
            {
                let has_sinks_update = pw_events.iter().any(|e| matches!(e, DaemonEvent::SinksUpdated(_)));
                if has_sinks_update {
                    let was_running = app.word_detector_status == WordDetectorStatus::Running;
                    app.try_autostart_detector();
                    autostarted = !was_running && app.word_detector_status == WordDetectorStatus::Running;
                }
            }
            for event in &pw_events {
                update_tray_np(&tray_now_playing, event);
            }
            broadcast(&client_senders, &pw_events);
            #[cfg(feature = "transcriber")]
            if autostarted {
                broadcast(&client_senders, &[DaemonEvent::State(app.snapshot())]);
            }
        }

        // Transcriber: spawn download thread if needed, poll detector matches
        #[cfg(feature = "transcriber")]
        {
            if app.word_detector_status == WordDetectorStatus::Downloading && !download_spawned {
                download_spawned = true;
                crate::log::log_info("Spawning model download thread");
                let dl_cmd_tx = cmd_tx.clone();
                std::thread::spawn(move || {
                    match download_model() {
                        Ok(()) => {
                            crate::log::log_info("Download thread: sending ModelDownloadComplete");
                            let _ = dl_cmd_tx.send(ClientCommand::ModelDownloadComplete);
                        }
                        Err(e) => {
                            crate::log::log_error(&format!("Download thread failed: {e:#}"));
                            let _ = dl_cmd_tx
                                .send(ClientCommand::ModelDownloadFailed(e.to_string()));
                        }
                    }
                });
            }
            if app.word_detector_status != WordDetectorStatus::Downloading {
                download_spawned = false;
            }

            let det_events = app.poll_detector_matches();
            if !det_events.is_empty() {
                for event in &det_events {
                    update_tray_np(&tray_now_playing, event);
                }
                broadcast(&client_senders, &det_events);
            }
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

#[cfg(feature = "transcriber")]
fn download_model() -> anyhow::Result<()> {
    use crate::protocol::{default_model_dir, MODEL_ASSET_NAME, MODEL_REPO};

    crate::log::log_info("Model download started");

    let model_dir = default_model_dir();
    crate::log::log_info(&format!("Model directory: {}", model_dir.display()));
    std::fs::create_dir_all(&model_dir)
        .with_context(|| format!("Failed to create model directory: {}", model_dir.display()))?;

    // Query GitHub API for latest release
    let api_url = format!(
        "https://api.github.com/repos/{MODEL_REPO}/releases/latest"
    );
    crate::log::log_info(&format!("Querying GitHub API: {}", api_url));
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(120)))
            .build(),
    );
    let body = agent.get(&api_url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "plentysound")
        .call()
        .context("Failed to query GitHub releases API")?
        .body_mut()
        .read_to_string()
        .context("Failed to read GitHub releases response")?;
    crate::log::log_info(&format!("GitHub API response length: {} bytes", body.len()));

    let response: serde_json::Value = serde_json::from_str(&body)
        .context("Failed to parse GitHub releases response")?;

    // Find the matching asset
    let assets = response["assets"]
        .as_array()
        .context("No assets in release")?;

    crate::log::log_info(&format!(
        "Found {} assets, looking for '{}'",
        assets.len(),
        MODEL_ASSET_NAME
    ));

    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n == MODEL_ASSET_NAME)
        })
        .context(format!("Asset '{}' not found in latest release", MODEL_ASSET_NAME))?;

    let download_url = asset["browser_download_url"]
        .as_str()
        .context("No download URL for asset")?;

    crate::log::log_info(&format!("Downloading asset from: {}", download_url));

    // Download the asset (5 min timeout for large files)
    let dest_file = model_dir.join(MODEL_ASSET_NAME);
    let dl_agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(300)))
            .build(),
    );
    let mut response = dl_agent.get(download_url)
        .header("User-Agent", "plentysound")
        .call()
        .context("Failed to download model asset")?;

    let mut file = std::fs::File::create(&dest_file)
        .with_context(|| format!("Failed to create file: {}", dest_file.display()))?;
    let bytes_written = std::io::copy(&mut response.body_mut().as_reader(), &mut file)
        .context("Failed to write downloaded file")?;
    drop(file);
    crate::log::log_info(&format!(
        "Downloaded {} bytes to {}",
        bytes_written,
        dest_file.display()
    ));

    // Extract: determine extraction method by file extension
    let asset_name = MODEL_ASSET_NAME;
    crate::log::log_info(&format!("Extracting {} into {}", asset_name, model_dir.display()));
    let extract_result = if asset_name.ends_with(".tar.zst") || asset_name.ends_with(".tar.zstd") {
        std::process::Command::new("tar")
            .args(["--zstd", "-xf"])
            .arg(&dest_file)
            .arg("-C")
            .arg(&model_dir)
            .status()
    } else if asset_name.ends_with(".tar.gz") || asset_name.ends_with(".tgz") {
        std::process::Command::new("tar")
            .args(["-xzf"])
            .arg(&dest_file)
            .arg("-C")
            .arg(&model_dir)
            .status()
    } else if asset_name.ends_with(".zip") {
        std::process::Command::new("unzip")
            .arg("-o")
            .arg(&dest_file)
            .arg("-d")
            .arg(&model_dir)
            .status()
    } else {
        anyhow::bail!("Unsupported archive format: {}", asset_name);
    };

    let status = extract_result.context("Failed to run extraction command")?;
    if !status.success() {
        anyhow::bail!("Extraction failed with status: {}", status);
    }
    crate::log::log_info("Extraction complete");

    // Clean up the archive file
    let _ = std::fs::remove_file(&dest_file);
    crate::log::log_info("Model download finished successfully");

    Ok(())
}
