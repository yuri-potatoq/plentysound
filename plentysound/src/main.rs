mod app;
mod audio;
mod client;
mod daemon;
mod event;
mod filebrowser;
mod log;
mod pipewire;
mod protocol;
mod tray;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("daemon") => daemon::run_daemon(),
        Some("stop") => client::send_stop(),
        _ => client::run_or_start(),
    }
}
