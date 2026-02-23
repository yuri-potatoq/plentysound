use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
pub enum ClientCommand {
    GetState,
    SelectSink(usize),
    SelectSong(usize),
    Play,
    SetVolume(f32),
    SetComfortNoise(f32),
    SetEqMidBoost(f32),
    AddSong(String),
    RemoveSong(usize),
    RefreshSinks,
    Quit,
    #[cfg(feature = "transcriber")]
    StartModelDownload,
    #[cfg(feature = "transcriber")]
    AddWordMapping {
        word: String,
        song_index: usize,
        source_description: String,
        output_description: String,
    },
    #[cfg(feature = "transcriber")]
    RemoveWordMapping(usize),
    #[cfg(feature = "transcriber")]
    StartWordDetector(u32),
    #[cfg(feature = "transcriber")]
    StopWordDetector,
    #[cfg(feature = "transcriber")]
    ModelDownloadComplete,
    #[cfg(feature = "transcriber")]
    ModelDownloadFailed(String),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SinkInfo {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub kind: String, // "Output" or "Input"
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SongInfo {
    pub path: String,
    pub name: String,
}

#[cfg(feature = "transcriber")]
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub enum WordDetectorStatus {
    #[default]
    Unavailable,
    Downloading,
    DownloadFailed(String),
    Ready,
    Running,
}

#[cfg(feature = "transcriber")]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WordMapping {
    pub word: String,
    pub song_name: String,
    pub song_path: String,
    #[serde(default)]
    pub source_description: String,
    #[serde(default)]
    pub output_description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DaemonState {
    pub sinks: Vec<SinkInfo>,
    pub songs: Vec<SongInfo>,
    pub selected_sink: usize,
    pub selected_song: usize,
    pub volume: f32,
    pub comfort_noise: f32,
    pub eq_mid_boost: f32,
    pub now_playing: Option<String>,
    #[cfg(feature = "transcriber")]
    #[serde(default)]
    pub word_detector_status: WordDetectorStatus,
    #[cfg(feature = "transcriber")]
    #[serde(default)]
    pub word_mappings: Vec<WordMapping>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DaemonEvent {
    State(DaemonState),
    SinksUpdated(Vec<SinkInfo>),
    PlaybackFinished,
    NowPlaying(Option<String>),
    Shutdown,
    #[cfg(feature = "transcriber")]
    WordDetected(String),
}

pub fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("plentysound.sock")
}

#[cfg(feature = "transcriber")]
pub const MODEL_REPO: &str = "yuri-potatoq/plentysound-vosk-models";
#[cfg(feature = "transcriber")]
pub const MODEL_ASSET_NAME: &str = "vosk-model-small-pt-0.3.tar.zst";
#[cfg(feature = "transcriber")]
pub const MODEL_SUBDIR: &str = "vosk-model-small-pt-0.3";

#[cfg(feature = "transcriber")]
pub fn default_model_dir() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".local/share")
        });
    data_dir.join("plentysound").join("models")
}

#[cfg(feature = "transcriber")]
pub fn model_path() -> PathBuf {
    default_model_dir().join(MODEL_SUBDIR)
}

pub fn send_message<T: Serialize>(stream: &mut impl Write, msg: &T) -> std::io::Result<()> {
    let json = serde_json::to_vec(msg).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let len = (json.len() as u32).to_le_bytes();
    stream.write_all(&len)?;
    stream.write_all(&json)?;
    stream.flush()
}

pub fn recv_message<T: DeserializeOwned>(stream: &mut impl Read) -> std::io::Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
