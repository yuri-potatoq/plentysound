use crate::pipewire::{DeviceKind, PwCommand, PwEvent, PwSink};
use crate::protocol::{ClientCommand, DaemonEvent, DaemonState, SinkInfo, SongInfo};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

#[derive(Debug, Clone)]
pub struct Song {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Config {
    songs: Vec<String>,
}

impl Config {
    fn path() -> PathBuf {
        let mut p = dirs_fallback_config_dir();
        p.push("plentysound");
        p.push("config.yaml");
        p
    }

    fn load() -> Self {
        let path = Self::path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_yaml::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(yaml) = serde_yaml::to_string(self) {
            let _ = std::fs::write(&path, yaml);
        }
    }
}

fn dirs_fallback_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(dir)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config")
    } else {
        PathBuf::from(".config")
    }
}

pub struct DaemonApp {
    pub sinks: Vec<PwSink>,
    pub selected_sink: usize,
    pub songs: Vec<Song>,
    pub selected_song: usize,
    pub volume: f32,
    pub comfort_noise: f32,
    pub eq_mid_boost: f32,
    pub now_playing: Option<String>,
    pub pw_cmd_tx: Sender<PwCommand>,
    pub pw_evt_rx: Receiver<PwEvent>,
}

impl DaemonApp {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (evt_tx, evt_rx) = std::sync::mpsc::channel();

        crate::pipewire::spawn_pw_thread(cmd_rx, evt_tx);

        let config = Config::load();
        let songs: Vec<Song> = config
            .songs
            .iter()
            .filter_map(|p| {
                let path = PathBuf::from(p);
                if path.exists() {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    Some(Song { path, name })
                } else {
                    None
                }
            })
            .collect();

        DaemonApp {
            sinks: Vec::new(),
            selected_sink: 0,
            songs,
            selected_song: 0,
            volume: 1.0,
            comfort_noise: 0.01,
            eq_mid_boost: 1.5,
            now_playing: None,
            pw_cmd_tx: cmd_tx,
            pw_evt_rx: evt_rx,
        }
    }

    fn save_config(&self) {
        let config = Config {
            songs: self
                .songs
                .iter()
                .map(|s| s.path.display().to_string())
                .collect(),
        };
        config.save();
    }

    pub fn process_pw_events(&mut self) -> Vec<DaemonEvent> {
        let mut events = Vec::new();
        while let Ok(evt) = self.pw_evt_rx.try_recv() {
            match evt {
                PwEvent::SinksUpdated(new_sinks) => {
                    self.sinks = new_sinks;
                    if self.selected_sink >= self.sinks.len() && !self.sinks.is_empty() {
                        self.selected_sink = self.sinks.len() - 1;
                    }
                    events.push(DaemonEvent::SinksUpdated(self.sinks_to_info()));
                }
                PwEvent::PlaybackFinished => {
                    self.now_playing = None;
                    events.push(DaemonEvent::PlaybackFinished);
                    events.push(DaemonEvent::NowPlaying(None));
                }
            }
        }
        events
    }

    pub fn apply_command(&mut self, cmd: ClientCommand) -> Vec<DaemonEvent> {
        match cmd {
            ClientCommand::GetState => {
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::SelectSink(idx) => {
                if idx < self.sinks.len() {
                    self.selected_sink = idx;
                }
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::SelectSong(idx) => {
                if idx < self.songs.len() {
                    self.selected_song = idx;
                }
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::Play => {
                self.play_selected_song();
                vec![DaemonEvent::NowPlaying(self.now_playing.clone())]
            }
            ClientCommand::SetVolume(v) => {
                self.volume = v.clamp(0.0, 5.0);
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::SetComfortNoise(v) => {
                self.comfort_noise = v.clamp(0.0, 0.05);
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::SetEqMidBoost(v) => {
                self.eq_mid_boost = v.clamp(0.0, 3.0);
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::AddSong(path_str) => {
                let path = PathBuf::from(&path_str);
                if path.exists() {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    self.songs.push(Song { path, name });
                    self.save_config();
                }
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::RemoveSong(idx) => {
                if idx < self.songs.len() {
                    self.songs.remove(idx);
                    if self.selected_song >= self.songs.len() && !self.songs.is_empty() {
                        self.selected_song = self.songs.len() - 1;
                    }
                    self.save_config();
                }
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::RefreshSinks => {
                let _ = self.pw_cmd_tx.send(PwCommand::ListSinks);
                vec![]
            }
            ClientCommand::Quit => {
                vec![DaemonEvent::Shutdown]
            }
        }
    }

    pub fn snapshot(&self) -> DaemonState {
        DaemonState {
            sinks: self.sinks_to_info(),
            songs: self
                .songs
                .iter()
                .map(|s| SongInfo {
                    path: s.path.display().to_string(),
                    name: s.name.clone(),
                })
                .collect(),
            selected_sink: self.selected_sink,
            selected_song: self.selected_song,
            volume: self.volume,
            comfort_noise: self.comfort_noise,
            eq_mid_boost: self.eq_mid_boost,
            now_playing: self.now_playing.clone(),
        }
    }

    fn sinks_to_info(&self) -> Vec<SinkInfo> {
        self.sinks
            .iter()
            .map(|s| SinkInfo {
                id: s.id,
                name: s.name.clone(),
                description: s.description.clone(),
                kind: match s.kind {
                    DeviceKind::Output => "Output".to_string(),
                    DeviceKind::Input => "Input".to_string(),
                },
            })
            .collect()
    }

    fn play_selected_song(&mut self) {
        if self.songs.is_empty() || self.sinks.is_empty() {
            return;
        }

        let song = &self.songs[self.selected_song];
        let sink = &self.sinks[self.selected_sink];

        match crate::audio::decode_file(&song.path) {
            Ok(decoded) => {
                self.now_playing = Some(song.name.clone());
                let _ = self.pw_cmd_tx.send(PwCommand::Play {
                    sink_id: sink.id,
                    kind: sink.kind,
                    node_name: sink.name.clone(),
                    samples: decoded.samples,
                    sample_rate: decoded.sample_rate,
                    channels: decoded.channels,
                    volume: self.volume,
                    comfort_noise: self.comfort_noise,
                    eq_mid_boost: self.eq_mid_boost,
                });
            }
            Err(e) => {
                crate::log::log_error(&format!("Failed to decode {}: {e}", song.name));
            }
        }
    }
}
