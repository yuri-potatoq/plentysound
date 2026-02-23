use crate::pipewire::{DeviceKind, PwCommand, PwEvent, PwSink};
use crate::protocol::{ClientCommand, DaemonEvent, DaemonState, SinkInfo, SongInfo};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

#[cfg(feature = "transcriber")]
use crate::protocol::{WordDetectorStatus, WordMapping};

#[derive(Debug, Clone)]
pub struct Song {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Config {
    songs: Vec<String>,
    #[serde(default = "default_volume")]
    volume: f32,
    #[serde(default = "default_comfort_noise")]
    comfort_noise: f32,
    #[serde(default = "default_eq_mid_boost")]
    eq_mid_boost: f32,
    #[cfg(feature = "transcriber")]
    #[serde(default)]
    word_mappings: Vec<WordMappingConfig>,
}

fn default_volume() -> f32 { 1.0 }
fn default_comfort_noise() -> f32 { 0.01 }
fn default_eq_mid_boost() -> f32 { 1.5 }

#[cfg(feature = "transcriber")]
#[derive(Serialize, Deserialize, Clone)]
struct WordMappingConfig {
    word: String,
    song_path: String,
    #[serde(default)]
    source_description: String,
    #[serde(default)]
    output_description: String,
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
    #[cfg(feature = "transcriber")]
    pub word_mappings: Vec<WordMapping>,
    #[cfg(feature = "transcriber")]
    pub word_detector_status: WordDetectorStatus,
    #[cfg(feature = "transcriber")]
    pub detector_stop_tx: Option<std::sync::mpsc::Sender<()>>,
    #[cfg(feature = "transcriber")]
    pub detector_match_rx: Option<std::sync::mpsc::Receiver<String>>,
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

        #[cfg(feature = "transcriber")]
        let word_mappings = Self::load_word_mappings(&config, &songs);
        #[cfg(feature = "transcriber")]
        crate::log::log_info(&format!("Loaded {} word mappings from config", word_mappings.len()));

        #[cfg(feature = "transcriber")]
        let word_detector_status = if crate::protocol::model_path().exists() {
            WordDetectorStatus::Ready
        } else {
            WordDetectorStatus::Unavailable
        };

        DaemonApp {
            sinks: Vec::new(),
            selected_sink: 0,
            songs,
            selected_song: 0,
            volume: config.volume,
            comfort_noise: config.comfort_noise,
            eq_mid_boost: config.eq_mid_boost,
            now_playing: None,
            pw_cmd_tx: cmd_tx,
            pw_evt_rx: evt_rx,
            #[cfg(feature = "transcriber")]
            word_mappings,
            #[cfg(feature = "transcriber")]
            word_detector_status,
            #[cfg(feature = "transcriber")]
            detector_stop_tx: None,
            #[cfg(feature = "transcriber")]
            detector_match_rx: None,
        }
    }

    #[cfg(feature = "transcriber")]
    fn load_word_mappings(config: &Config, songs: &[Song]) -> Vec<WordMapping> {
        config
            .word_mappings
            .iter()
            .filter_map(|wm| {
                let song = songs
                    .iter()
                    .find(|s| s.path.display().to_string() == wm.song_path)?;
                Some(WordMapping {
                    word: wm.word.clone(),
                    song_name: song.name.clone(),
                    song_path: wm.song_path.clone(),
                    source_description: wm.source_description.clone(),
                    output_description: wm.output_description.clone(),
                })
            })
            .collect()
    }

    fn save_config(&self) {
        let config = Config {
            songs: self
                .songs
                .iter()
                .map(|s| s.path.display().to_string())
                .collect(),
            volume: self.volume,
            comfort_noise: self.comfort_noise,
            eq_mid_boost: self.eq_mid_boost,
            #[cfg(feature = "transcriber")]
            word_mappings: self
                .word_mappings
                .iter()
                .map(|wm| WordMappingConfig {
                    word: wm.word.clone(),
                    song_path: wm.song_path.clone(),
                    source_description: wm.source_description.clone(),
                    output_description: wm.output_description.clone(),
                })
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
                self.save_config();
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::SetComfortNoise(v) => {
                self.comfort_noise = v.clamp(0.0, 0.05);
                self.save_config();
                vec![DaemonEvent::State(self.snapshot())]
            }
            ClientCommand::SetEqMidBoost(v) => {
                self.eq_mid_boost = v.clamp(0.0, 3.0);
                self.save_config();
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
            #[cfg(feature = "transcriber")]
            ClientCommand::StartModelDownload => {
                self.word_detector_status = WordDetectorStatus::Downloading;
                vec![DaemonEvent::State(self.snapshot())]
            }
            #[cfg(feature = "transcriber")]
            ClientCommand::AddWordMapping { word, song_index, source_description, output_description } => {
                if song_index < self.songs.len() {
                    let song = &self.songs[song_index];
                    self.word_mappings.push(WordMapping {
                        word,
                        song_name: song.name.clone(),
                        song_path: song.path.display().to_string(),
                        source_description,
                        output_description,
                    });
                    self.save_config();
                }
                vec![DaemonEvent::State(self.snapshot())]
            }
            #[cfg(feature = "transcriber")]
            ClientCommand::RemoveWordMapping(idx) => {
                if idx < self.word_mappings.len() {
                    self.word_mappings.remove(idx);
                    self.save_config();
                }
                vec![DaemonEvent::State(self.snapshot())]
            }
            #[cfg(feature = "transcriber")]
            ClientCommand::StartWordDetector(node_id) => {
                self.start_detector(node_id);
                vec![DaemonEvent::State(self.snapshot())]
            }
            #[cfg(feature = "transcriber")]
            ClientCommand::StopWordDetector => {
                self.stop_detector();
                vec![DaemonEvent::State(self.snapshot())]
            }
            #[cfg(feature = "transcriber")]
            ClientCommand::ModelDownloadComplete => {
                crate::log::log_info("ModelDownloadComplete: setting status to Ready");
                self.word_detector_status = WordDetectorStatus::Ready;
                let snap = self.snapshot();
                crate::log::log_info(&format!(
                    "ModelDownloadComplete: snapshot status = {:?}",
                    snap.word_detector_status
                ));
                vec![DaemonEvent::State(snap)]
            }
            #[cfg(feature = "transcriber")]
            ClientCommand::ModelDownloadFailed(msg) => {
                self.word_detector_status = WordDetectorStatus::DownloadFailed(msg);
                vec![DaemonEvent::State(self.snapshot())]
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
            #[cfg(feature = "transcriber")]
            word_detector_status: self.word_detector_status.clone(),
            #[cfg(feature = "transcriber")]
            word_mappings: self.word_mappings.clone(),
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

    #[cfg(feature = "transcriber")]
    pub fn play_song_by_path(&mut self, song_path: &str) {
        let song_idx = self
            .songs
            .iter()
            .position(|s| s.path.display().to_string() == song_path);
        if let Some(idx) = song_idx {
            self.selected_song = idx;
            self.play_selected_song();
        }
    }

    /// Try to auto-start the detector if the model is ready, there are word
    /// mappings, and we can find a matching input source among discovered sinks.
    #[cfg(feature = "transcriber")]
    pub fn try_autostart_detector(&mut self) {
        if self.word_detector_status != WordDetectorStatus::Ready {
            return;
        }
        if self.word_mappings.is_empty() || self.sinks.is_empty() {
            return;
        }
        // Already running
        if self.detector_stop_tx.is_some() {
            return;
        }

        // Try to match the source_description from the first mapping that has one
        let saved_desc = self.word_mappings.iter()
            .map(|wm| wm.source_description.as_str())
            .find(|d| !d.is_empty());

        let input_node = if let Some(desc) = saved_desc {
            // Prefer the saved source
            self.sinks.iter()
                .find(|s| s.kind == crate::pipewire::DeviceKind::Input && s.description == desc)
                .or_else(|| self.sinks.iter().find(|s| s.kind == crate::pipewire::DeviceKind::Input))
        } else {
            // Fallback: first available input
            self.sinks.iter().find(|s| s.kind == crate::pipewire::DeviceKind::Input)
        };

        if let Some(node) = input_node {
            let node_id = node.id;
            crate::log::log_info(&format!(
                "Auto-starting detector with input node {} ({})",
                node_id, node.description
            ));
            self.start_detector(node_id);
        }
    }

    #[cfg(feature = "transcriber")]
    fn start_detector(&mut self, node_id: u32) {
        crate::log::log_info(&format!("start_detector called with node_id={}", node_id));
        self.stop_detector();

        let model = crate::protocol::model_path();
        let model_str = model.display().to_string();
        let keywords: Vec<String> = self.word_mappings.iter().map(|wm| wm.word.clone()).collect();

        if keywords.is_empty() {
            crate::log::log_info("start_detector: no keywords, returning");
            return;
        }

        crate::log::log_info(&format!(
            "Starting detector: model={}, keywords={:?}, node={}",
            model_str, keywords, node_id
        ));

        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let (match_tx, match_rx) = std::sync::mpsc::channel();

        self.detector_stop_tx = Some(stop_tx);
        self.detector_match_rx = Some(match_rx);
        self.word_detector_status = WordDetectorStatus::Running;

        std::thread::spawn(move || {
            crate::log::log_info("Detector thread started");
            if let Err(e) = plentysound_transcriber::detector::run_detector(
                &model_str,
                &keywords,
                node_id,
                stop_rx,
                move |word| {
                    crate::log::log_info(&format!("Detector matched word: \"{}\"", word));
                    let _ = match_tx.send(word);
                },
                |msg| {
                    crate::log::log_info(msg);
                },
            ) {
                crate::log::log_error(&format!("Detector error: {e:#}"));
            }
            crate::log::log_info("Detector thread exiting");
        });
    }

    #[cfg(feature = "transcriber")]
    fn stop_detector(&mut self) {
        crate::log::log_info("stop_detector called");
        if let Some(tx) = self.detector_stop_tx.take() {
            let _ = tx.send(());
        }
        self.detector_match_rx = None;
        if self.word_detector_status == WordDetectorStatus::Running {
            self.word_detector_status = WordDetectorStatus::Ready;
        }
    }

    #[cfg(feature = "transcriber")]
    pub fn poll_detector_matches(&mut self) -> Vec<DaemonEvent> {
        // Drain all matches first to release the borrow on self
        let words: Vec<String> = self
            .detector_match_rx
            .as_ref()
            .map(|rx| {
                let mut v = Vec::new();
                while let Ok(word) = rx.try_recv() {
                    v.push(word);
                }
                v
            })
            .unwrap_or_default();

        let mut events = Vec::new();
        for word in words {
            let mapping = self
                .word_mappings
                .iter()
                .find(|wm| wm.word == word)
                .cloned();
            if let Some(mapping) = mapping {
                self.play_song_by_path(&mapping.song_path);
                events.push(DaemonEvent::WordDetected(word));
            }
        }
        events
    }
}
