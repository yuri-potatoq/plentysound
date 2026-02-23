use crate::filebrowser::FileBrowser;
use crate::protocol::{
    socket_path, ClientCommand, DaemonEvent, DaemonState, SinkInfo, SongInfo,
    recv_message, send_message,
};
use anyhow::{Context, Result};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::{execute, terminal};
use ratatui::layout::Rect;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[cfg(feature = "transcriber")]
use crate::textinput::TextInput;
#[cfg(feature = "transcriber")]
use crate::protocol::WordDetectorStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Sinks,
    Volume,
    AudioFx,
    AddButton,
    #[cfg(feature = "transcriber")]
    WordDetectorButton,
    Songs,
    #[cfg(feature = "transcriber")]
    WordBindings,
}

#[derive(Default, Clone, Copy)]
pub struct AppLayout {
    pub sinks_area: Rect,
    pub volume_area: Rect,
    pub audio_fx_area: Rect,
    pub add_button_area: Rect,
    #[cfg(feature = "transcriber")]
    pub word_detector_button_area: Rect,
    pub songs_area: Rect,
    #[cfg(feature = "transcriber")]
    pub word_bindings_area: Rect,
}

#[cfg(feature = "transcriber")]
pub enum TranscriberOverlay {
    SelectSource { selected: usize },
    SelectOutput { selected: usize },
    EnterWord { input: TextInput },
    PickSong { word: String, selected: usize },
}

pub struct ClientApp {
    pub state: DaemonState,
    pub focus: Panel,
    pub selected_fx: usize,
    pub file_browser: Option<FileBrowser>,
    #[cfg(feature = "transcriber")]
    pub transcriber_overlay: Option<TranscriberOverlay>,
    #[cfg(feature = "transcriber")]
    pub detector_source_node: Option<u32>,
    #[cfg(feature = "transcriber")]
    pub detector_source_description: Option<String>,
    #[cfg(feature = "transcriber")]
    pub detector_output_description: Option<String>,
    #[cfg(feature = "transcriber")]
    pub selected_word_binding: usize,
    pub layout: AppLayout,
    pub should_quit: bool,
    pub status_message: Option<String>,
    stream: UnixStream,
}

impl ClientApp {
    fn new(mut stream: UnixStream) -> Result<Self> {
        let event: DaemonEvent = recv_message(&mut stream)
            .context("Failed to receive initial state from daemon")?;
        let state = match event {
            DaemonEvent::State(s) => s,
            _ => anyhow::bail!("Expected State event from daemon, got {:?}", event),
        };

        stream.set_nonblocking(true)?;

        Ok(ClientApp {
            state,
            focus: Panel::Sinks,
            selected_fx: 0,
            file_browser: None,
            #[cfg(feature = "transcriber")]
            transcriber_overlay: None,
            #[cfg(feature = "transcriber")]
            detector_source_node: None,
            #[cfg(feature = "transcriber")]
            detector_source_description: None,
            #[cfg(feature = "transcriber")]
            detector_output_description: None,
            #[cfg(feature = "transcriber")]
            selected_word_binding: 0,
            layout: AppLayout::default(),
            should_quit: false,
            status_message: None,
            stream,
        })
    }

    fn send_command(&mut self, cmd: ClientCommand) {
        self.stream.set_nonblocking(false).ok();
        if let Err(e) = send_message(&mut self.stream, &cmd) {
            crate::log::log_error(&format!("Failed to send command: {e}"));
        }
        self.stream.set_nonblocking(true).ok();
    }

    fn poll_daemon_events(&mut self) {
        loop {
            match recv_message::<DaemonEvent>(&mut self.stream) {
                Ok(event) => match event {
                    DaemonEvent::State(s) => {
                        #[cfg(feature = "transcriber")]
                        {
                            crate::log::log_info(&format!(
                                "Client received State: detector_status={:?}",
                                s.word_detector_status
                            ));
                            if let WordDetectorStatus::DownloadFailed(ref msg) = s.word_detector_status {
                                self.status_message = Some(format!("Model download failed: {}", msg));
                            }
                        }
                        self.state = s;
                    }
                    DaemonEvent::SinksUpdated(sinks) => {
                        self.state.sinks = sinks;
                        if self.state.selected_sink >= self.state.sinks.len()
                            && !self.state.sinks.is_empty()
                        {
                            self.state.selected_sink = self.state.sinks.len() - 1;
                        }
                    }
                    DaemonEvent::PlaybackFinished => {
                        self.state.now_playing = None;
                    }
                    DaemonEvent::NowPlaying(np) => {
                        self.state.now_playing = np;
                    }
                    DaemonEvent::Shutdown => {
                        self.should_quit = true;
                        return;
                    }
                    #[cfg(feature = "transcriber")]
                    DaemonEvent::WordDetected(word) => {
                        self.status_message = Some(format!("Word detected: \"{}\"", word));
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.should_quit = true;
                    break;
                }
            }
        }
    }

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => {
                #[cfg(feature = "transcriber")]
                if self.transcriber_overlay.is_some() {
                    self.handle_overlay_key(key);
                    return;
                }
                if self.file_browser.is_some() {
                    self.handle_filebrowser_key(key);
                } else {
                    self.handle_main_key(key);
                }
            }
            Event::Mouse(mouse) => {
                #[cfg(feature = "transcriber")]
                if self.transcriber_overlay.is_some() {
                    return;
                }
                if self.file_browser.is_none() {
                    self.handle_mouse(mouse);
                }
            }
            _ => {}
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        self.status_message = None;
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Tab => self.cycle_focus(),
            KeyCode::BackTab => self.cycle_focus_back(),
            KeyCode::Left => self.handle_left(),
            KeyCode::Right => self.handle_right(),
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Enter => self.activate(),
            KeyCode::Char('d') | KeyCode::Delete => self.delete_selected(),
            KeyCode::Char('r') => {
                self.send_command(ClientCommand::RefreshSinks);
            }
            _ => {}
        }
    }

    fn handle_filebrowser_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.file_browser = None;
            }
            KeyCode::Up => {
                if let Some(fb) = &mut self.file_browser {
                    fb.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(fb) = &mut self.file_browser {
                    fb.move_down();
                }
            }
            KeyCode::Enter => {
                let selected_path = self.file_browser.as_mut().and_then(|fb| fb.select());
                if let Some(path) = selected_path {
                    self.send_command(ClientCommand::AddSong(path.display().to_string()));
                    self.file_browser = None;
                }
            }
            KeyCode::Backspace => {
                if let Some(fb) = &mut self.file_browser {
                    fb.navigate_parent();
                }
            }
            _ => {}
        }
    }

    #[cfg(feature = "transcriber")]
    fn handle_overlay_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.transcriber_overlay = None;
            }
            _ => {
                let overlay = self.transcriber_overlay.take();
                match overlay {
                    Some(TranscriberOverlay::SelectSource { mut selected }) => {
                        let input_sinks: Vec<_> = self
                            .state
                            .sinks
                            .iter()
                            .filter(|s| s.kind == "Input")
                            .collect();
                        match key.code {
                            KeyCode::Up => {
                                if selected > 0 {
                                    selected -= 1;
                                }
                            }
                            KeyCode::Down => {
                                if !input_sinks.is_empty()
                                    && selected < input_sinks.len() - 1
                                {
                                    selected += 1;
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(sink) = input_sinks.get(selected) {
                                    self.detector_source_node = Some(sink.id);
                                    self.detector_source_description = Some(sink.description.clone());
                                    self.transcriber_overlay =
                                        Some(TranscriberOverlay::SelectOutput {
                                            selected: 0,
                                        });
                                    return;
                                }
                            }
                            _ => {}
                        }
                        self.transcriber_overlay =
                            Some(TranscriberOverlay::SelectSource { selected });
                    }
                    Some(TranscriberOverlay::SelectOutput { mut selected }) => {
                        let output_sinks: Vec<_> = self
                            .state
                            .sinks
                            .iter()
                            .filter(|s| s.kind == "Output")
                            .collect();
                        match key.code {
                            KeyCode::Up => {
                                if selected > 0 {
                                    selected -= 1;
                                }
                            }
                            KeyCode::Down => {
                                if !output_sinks.is_empty()
                                    && selected < output_sinks.len() - 1
                                {
                                    selected += 1;
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(sink) = output_sinks.get(selected) {
                                    self.detector_output_description = Some(sink.description.clone());
                                    // Select this output sink in the main app
                                    if let Some(idx) = self.state.sinks.iter().position(|s| s.id == sink.id) {
                                        self.send_command(ClientCommand::SelectSink(idx));
                                    }
                                    self.transcriber_overlay =
                                        Some(TranscriberOverlay::EnterWord {
                                            input: TextInput::new(),
                                        });
                                    return;
                                }
                            }
                            _ => {}
                        }
                        self.transcriber_overlay =
                            Some(TranscriberOverlay::SelectOutput { selected });
                    }
                    Some(TranscriberOverlay::EnterWord { mut input }) => {
                        match key.code {
                            KeyCode::Enter => {
                                if !input.is_empty() {
                                    let word = input.as_str().to_string();
                                    self.transcriber_overlay =
                                        Some(TranscriberOverlay::PickSong {
                                            word,
                                            selected: 0,
                                        });
                                    return;
                                }
                            }
                            KeyCode::Backspace => {
                                input.backspace();
                            }
                            KeyCode::Char(c) => {
                                input.push_char(c);
                            }
                            _ => {}
                        }
                        self.transcriber_overlay =
                            Some(TranscriberOverlay::EnterWord { input });
                    }
                    Some(TranscriberOverlay::PickSong {
                        word,
                        mut selected,
                    }) => {
                        match key.code {
                            KeyCode::Up => {
                                if selected > 0 {
                                    selected -= 1;
                                }
                            }
                            KeyCode::Down => {
                                if !self.state.songs.is_empty()
                                    && selected < self.state.songs.len() - 1
                                {
                                    selected += 1;
                                }
                            }
                            KeyCode::Enter => {
                                if selected < self.state.songs.len() {
                                    self.send_command(ClientCommand::AddWordMapping {
                                        word: word.clone(),
                                        song_index: selected,
                                        source_description: self.detector_source_description.clone().unwrap_or_default(),
                                        output_description: self.detector_output_description.clone().unwrap_or_default(),
                                    });
                                    // Start the detector with the selected source
                                    if let Some(node_id) = self.detector_source_node {
                                        self.send_command(
                                            ClientCommand::StartWordDetector(node_id),
                                        );
                                    }
                                    self.transcriber_overlay = None;
                                    self.status_message = Some(format!(
                                        "Mapped \"{}\" -> {}",
                                        word,
                                        self.state.songs[selected].name
                                    ));
                                    return;
                                }
                            }
                            _ => {}
                        }
                        self.transcriber_overlay =
                            Some(TranscriberOverlay::PickSong { word, selected });
                    }
                    None => {}
                }
            }
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }

        let col = mouse.column;
        let row = mouse.row;

        if self.layout.sinks_area.contains((col, row).into()) {
            self.focus = Panel::Sinks;
            let inner_y = row.saturating_sub(self.layout.sinks_area.y + 1);
            let idx = inner_y as usize;
            if idx < self.state.sinks.len() {
                self.send_command(ClientCommand::SelectSink(idx));
            }
        } else if self.layout.volume_area.contains((col, row).into()) {
            self.focus = Panel::Volume;
            let inner_x = col.saturating_sub(self.layout.volume_area.x + 1);
            let inner_width = self.layout.volume_area.width.saturating_sub(2);
            if inner_width > 0 {
                let vol = (inner_x as f32 / inner_width as f32 * 5.0).clamp(0.0, 5.0);
                self.state.volume = vol;
                self.send_command(ClientCommand::SetVolume(vol));
            }
        } else if self.layout.audio_fx_area.contains((col, row).into()) {
            self.focus = Panel::AudioFx;
            let inner_y = row.saturating_sub(self.layout.audio_fx_area.y + 1);
            let inner_x = col.saturating_sub(self.layout.audio_fx_area.x + 1);
            let inner_width = self.layout.audio_fx_area.width.saturating_sub(2);
            if inner_y < 2 {
                self.selected_fx = inner_y as usize;
                if inner_width > 0 {
                    let ratio = inner_x as f32 / inner_width as f32;
                    match self.selected_fx {
                        0 => {
                            let v = (ratio * 0.05).clamp(0.0, 0.05);
                            self.state.comfort_noise = v;
                            self.send_command(ClientCommand::SetComfortNoise(v));
                        }
                        1 => {
                            let v = (ratio * 3.0).clamp(0.0, 3.0);
                            self.state.eq_mid_boost = v;
                            self.send_command(ClientCommand::SetEqMidBoost(v));
                        }
                        _ => {}
                    }
                }
            }
        } else if self.layout.add_button_area.contains((col, row).into()) {
            self.focus = Panel::AddButton;
            self.activate();
        }
        #[cfg(feature = "transcriber")]
        if self.layout.word_detector_button_area.contains((col, row).into()) {
            self.focus = Panel::WordDetectorButton;
            self.activate();
            return;
        }
        #[cfg(feature = "transcriber")]
        if self.layout.word_bindings_area.contains((col, row).into()) {
            self.focus = Panel::WordBindings;
            let inner_y = row.saturating_sub(self.layout.word_bindings_area.y + 1);
            let bindings = self.bindings_for_selected_song();
            if !bindings.is_empty() {
                self.selected_word_binding = (inner_y as usize).min(bindings.len() - 1);
            }
            return;
        }
        if self.layout.songs_area.contains((col, row).into()) {
            self.focus = Panel::Songs;
            let inner_y = row.saturating_sub(self.layout.songs_area.y + 1);
            let idx = inner_y as usize;
            if !self.state.songs.is_empty() && idx < self.state.songs.len() {
                self.send_command(ClientCommand::SelectSong(idx));
                self.send_command(ClientCommand::Play);
            }
        }
    }

    fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Panel::Sinks => Panel::Volume,
            Panel::Volume => Panel::AudioFx,
            Panel::AudioFx => Panel::AddButton,
            #[cfg(feature = "transcriber")]
            Panel::AddButton => Panel::WordDetectorButton,
            #[cfg(feature = "transcriber")]
            Panel::WordDetectorButton => Panel::Songs,
            #[cfg(not(feature = "transcriber"))]
            Panel::AddButton => Panel::Songs,
            #[cfg(feature = "transcriber")]
            Panel::Songs => {
                if self.show_word_bindings_panel() {
                    Panel::WordBindings
                } else {
                    Panel::Sinks
                }
            }
            #[cfg(not(feature = "transcriber"))]
            Panel::Songs => Panel::Sinks,
            #[cfg(feature = "transcriber")]
            Panel::WordBindings => Panel::Sinks,
        };
    }

    fn cycle_focus_back(&mut self) {
        self.focus = match self.focus {
            #[cfg(feature = "transcriber")]
            Panel::Sinks => {
                if self.show_word_bindings_panel() {
                    Panel::WordBindings
                } else {
                    Panel::Songs
                }
            }
            #[cfg(not(feature = "transcriber"))]
            Panel::Sinks => Panel::Songs,
            Panel::Volume => Panel::Sinks,
            Panel::AudioFx => Panel::Volume,
            #[cfg(feature = "transcriber")]
            Panel::AddButton => Panel::AudioFx,
            #[cfg(feature = "transcriber")]
            Panel::WordDetectorButton => Panel::AddButton,
            #[cfg(feature = "transcriber")]
            Panel::Songs => Panel::WordDetectorButton,
            #[cfg(not(feature = "transcriber"))]
            Panel::AddButton => Panel::AudioFx,
            #[cfg(not(feature = "transcriber"))]
            Panel::Songs => Panel::AddButton,
            #[cfg(feature = "transcriber")]
            Panel::WordBindings => Panel::Songs,
        };
    }

    #[cfg(feature = "transcriber")]
    fn show_word_bindings_panel(&self) -> bool {
        matches!(
            self.state.word_detector_status,
            WordDetectorStatus::Ready | WordDetectorStatus::Running
        )
    }

    fn handle_left(&mut self) {
        match self.focus {
            Panel::Volume => {
                self.state.volume = (self.state.volume - 0.05).clamp(0.0, 5.0);
                self.send_command(ClientCommand::SetVolume(self.state.volume));
            }
            Panel::AudioFx => match self.selected_fx {
                0 => {
                    self.state.comfort_noise =
                        (self.state.comfort_noise - 0.005).clamp(0.0, 0.05);
                    self.send_command(ClientCommand::SetComfortNoise(self.state.comfort_noise));
                }
                1 => {
                    self.state.eq_mid_boost =
                        (self.state.eq_mid_boost - 0.1).clamp(0.0, 3.0);
                    self.send_command(ClientCommand::SetEqMidBoost(self.state.eq_mid_boost));
                }
                _ => {}
            },
            _ => self.cycle_focus_back(),
        }
    }

    fn handle_right(&mut self) {
        match self.focus {
            Panel::Volume => {
                self.state.volume = (self.state.volume + 0.05).clamp(0.0, 5.0);
                self.send_command(ClientCommand::SetVolume(self.state.volume));
            }
            Panel::AudioFx => match self.selected_fx {
                0 => {
                    self.state.comfort_noise =
                        (self.state.comfort_noise + 0.005).clamp(0.0, 0.05);
                    self.send_command(ClientCommand::SetComfortNoise(self.state.comfort_noise));
                }
                1 => {
                    self.state.eq_mid_boost =
                        (self.state.eq_mid_boost + 0.1).clamp(0.0, 3.0);
                    self.send_command(ClientCommand::SetEqMidBoost(self.state.eq_mid_boost));
                }
                _ => {}
            },
            _ => self.cycle_focus(),
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Panel::Sinks => {
                if self.state.selected_sink > 0 {
                    self.state.selected_sink -= 1;
                    self.send_command(ClientCommand::SelectSink(self.state.selected_sink));
                }
            }
            Panel::Songs => {
                if self.state.selected_song > 0 {
                    self.state.selected_song -= 1;
                    self.send_command(ClientCommand::SelectSong(self.state.selected_song));
                    #[cfg(feature = "transcriber")]
                    {
                        self.selected_word_binding = 0;
                    }
                }
            }
            Panel::AudioFx => {
                if self.selected_fx > 0 {
                    self.selected_fx -= 1;
                }
            }
            #[cfg(feature = "transcriber")]
            Panel::WordBindings => {
                if self.selected_word_binding > 0 {
                    self.selected_word_binding -= 1;
                }
            }
            _ => {}
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            Panel::Sinks => {
                if !self.state.sinks.is_empty()
                    && self.state.selected_sink < self.state.sinks.len() - 1
                {
                    self.state.selected_sink += 1;
                    self.send_command(ClientCommand::SelectSink(self.state.selected_sink));
                }
            }
            Panel::Songs => {
                if !self.state.songs.is_empty()
                    && self.state.selected_song < self.state.songs.len() - 1
                {
                    self.state.selected_song += 1;
                    self.send_command(ClientCommand::SelectSong(self.state.selected_song));
                    #[cfg(feature = "transcriber")]
                    {
                        self.selected_word_binding = 0;
                    }
                }
            }
            Panel::AudioFx => {
                if self.selected_fx < 1 {
                    self.selected_fx += 1;
                }
            }
            #[cfg(feature = "transcriber")]
            Panel::WordBindings => {
                let count = self.bindings_for_selected_song().len();
                if count > 0 && self.selected_word_binding < count - 1 {
                    self.selected_word_binding += 1;
                }
            }
            _ => {}
        }
    }

    fn activate(&mut self) {
        match self.focus {
            Panel::AddButton => {
                self.file_browser = Some(FileBrowser::new());
            }
            Panel::Songs => {
                self.send_command(ClientCommand::Play);
            }
            #[cfg(feature = "transcriber")]
            Panel::WordDetectorButton => {
                self.activate_word_detector();
            }
            _ => {}
        }
    }

    #[cfg(feature = "transcriber")]
    fn activate_word_detector(&mut self) {
        match &self.state.word_detector_status {
            WordDetectorStatus::Unavailable | WordDetectorStatus::DownloadFailed(_) => {
                self.send_command(ClientCommand::StartModelDownload);
                self.status_message = Some("Starting model download...".to_string());
            }
            WordDetectorStatus::Downloading => {
                self.status_message = Some("Model download in progress...".to_string());
            }
            WordDetectorStatus::Ready => {
                // Open source selection overlay
                self.transcriber_overlay =
                    Some(TranscriberOverlay::SelectSource { selected: 0 });
            }
            WordDetectorStatus::Running => {
                // Open overlay to add more mappings or stop
                self.transcriber_overlay =
                    Some(TranscriberOverlay::SelectSource { selected: 0 });
            }
        }
    }

    fn delete_selected(&mut self) {
        match self.focus {
            Panel::Songs => {
                if !self.state.songs.is_empty() {
                    self.send_command(ClientCommand::RemoveSong(self.state.selected_song));
                }
            }
            #[cfg(feature = "transcriber")]
            Panel::WordBindings => {
                let bindings = self.bindings_for_selected_song();
                let count = bindings.len();
                if let Some(&(global_idx, _)) = bindings.get(self.selected_word_binding) {
                    drop(bindings);
                    self.send_command(ClientCommand::RemoveWordMapping(global_idx));
                    if self.selected_word_binding > 0
                        && self.selected_word_binding >= count - 1
                    {
                        self.selected_word_binding -= 1;
                    }
                }
            }
            _ => {}
        }
    }

    #[cfg(feature = "transcriber")]
    pub fn bindings_for_selected_song(&self) -> Vec<(usize, &crate::protocol::WordMapping)> {
        if self.state.songs.is_empty() {
            return Vec::new();
        }
        let selected_path = &self.state.songs[self.state.selected_song].path;
        self.state
            .word_mappings
            .iter()
            .enumerate()
            .filter(|(_, wm)| wm.song_path == *selected_path)
            .collect()
    }

    // Accessors for UI compatibility
    pub fn sinks(&self) -> &[SinkInfo] {
        &self.state.sinks
    }
    pub fn songs(&self) -> &[SongInfo] {
        &self.state.songs
    }
    pub fn selected_sink(&self) -> usize {
        self.state.selected_sink
    }
    pub fn selected_song(&self) -> usize {
        self.state.selected_song
    }
    pub fn volume(&self) -> f32 {
        self.state.volume
    }
    pub fn comfort_noise(&self) -> f32 {
        self.state.comfort_noise
    }
    pub fn eq_mid_boost(&self) -> f32 {
        self.state.eq_mid_boost
    }
    pub fn now_playing(&self) -> Option<&str> {
        self.state.now_playing.as_deref()
    }
}

fn connect_to_daemon() -> Result<UnixStream> {
    let path = socket_path();
    UnixStream::connect(&path).with_context(|| format!("Cannot connect to daemon at {}", path.display()))
}

fn spawn_daemon() -> Result<()> {
    let exe = std::env::current_exe().context("Cannot determine own executable path")?;
    let log_file = crate::log::open_log_file();
    let stderr_cfg = match log_file {
        Some(f) => std::process::Stdio::from(f),
        None => std::process::Stdio::null(),
    };
    std::process::Command::new(exe)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr_cfg)
        .spawn()
        .context("Failed to spawn daemon process")?;
    Ok(())
}

pub fn run_or_start() -> Result<()> {
    // Try connecting to existing daemon
    let stream = match connect_to_daemon() {
        Ok(s) => s,
        Err(_) => {
            // No daemon running, spawn one
            spawn_daemon()?;
            // Wait for socket to appear
            let path = socket_path();
            let mut connected = None;
            for _ in 0..50 {
                std::thread::sleep(Duration::from_millis(100));
                if let Ok(s) = UnixStream::connect(&path) {
                    connected = Some(s);
                    break;
                }
            }
            connected.context("Daemon did not start in time")?
        }
    };

    let mut app = ClientApp::new(stream)?;
    run_tui(&mut app)
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        terminal::LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_tui(app: &mut ClientApp) -> Result<()> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            terminal::LeaveAlternateScreen,
            DisableMouseCapture
        );
        original_hook(info);
    }));

    let mut terminal = setup_terminal()?;

    loop {
        terminal.draw(|f| crate::ui::draw(f, app))?;

        if let Some(ev) = crate::event::poll_event(Duration::from_millis(50)) {
            app.handle_event(ev);
        }

        app.poll_daemon_events();

        if app.should_quit {
            break;
        }
    }

    restore_terminal(&mut terminal)?;
    Ok(())
}

pub fn send_stop() -> Result<()> {
    let mut stream = connect_to_daemon().context("No daemon is running")?;
    stream.set_nonblocking(false)?;
    // Must read the initial State the daemon sends on connect,
    // otherwise the daemon's handle_new_client bails before spawning
    // the reader thread and our Quit command is never processed.
    let _initial: DaemonEvent = recv_message(&mut stream)
        .context("Failed to receive initial state from daemon")?;
    send_message(&mut stream, &ClientCommand::Quit)?;
    println!("Sent stop signal to daemon.");
    Ok(())
}
