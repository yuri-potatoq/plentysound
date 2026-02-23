use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOG_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn log_path() -> PathBuf {
    let mut path = if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(dir)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share")
    } else {
        PathBuf::from(".")
    };
    path.push("plentysound");
    let _ = std::fs::create_dir_all(&path);
    path.push("plentysound.log");
    path
}

pub fn open_log_file() -> Option<std::fs::File> {
    let path = log_path();
    OpenOptions::new().create(true).append(true).open(&path).ok()
}

pub fn log_info(msg: &str) {
    log_write("INFO", msg);
}

pub fn log_error(msg: &str) {
    log_write("ERROR", msg);
}

fn log_write(level: &str, msg: &str) {
    let path = {
        let mut cached = LOG_FILE.lock().unwrap();
        if cached.is_none() {
            *cached = Some(log_path());
        }
        cached.clone().unwrap()
    };

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[{timestamp}] [{level}] {msg}");
    }
}
