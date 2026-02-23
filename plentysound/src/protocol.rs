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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DaemonEvent {
    State(DaemonState),
    SinksUpdated(Vec<SinkInfo>),
    PlaybackFinished,
    NowPlaying(Option<String>),
    Shutdown,
}

pub fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("plentysound.sock")
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
