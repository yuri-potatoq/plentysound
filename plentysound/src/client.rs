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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Sinks,
    Volume,
    AudioFx,
    AddButton,
    Songs,
}

#[derive(Default, Clone, Copy)]
pub struct AppLayout {
    pub sinks_area: Rect,
    pub volume_area: Rect,
    pub audio_fx_area: Rect,
    pub add_button_area: Rect,
    pub songs_area: Rect,
}

pub struct ClientApp {
    pub state: DaemonState,
    pub focus: Panel,
    pub selected_fx: usize,
    pub file_browser: Option<FileBrowser>,
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
                if self.file_browser.is_some() {
                    self.handle_filebrowser_key(key);
                } else {
                    self.handle_main_key(key);
                }
            }
            Event::Mouse(mouse) => {
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
            KeyCode::Char('d') | KeyCode::Delete => self.delete_selected_song(),
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
        } else if self.layout.songs_area.contains((col, row).into()) {
            self.focus = Panel::Songs;
            let inner_y = row.saturating_sub(self.layout.songs_area.y + 1);
            let idx = inner_y as usize;
            if !self.state.songs.is_empty() {
                let idx = idx.min(self.state.songs.len() - 1);
                self.send_command(ClientCommand::SelectSong(idx));
            }
            self.activate();
        }
    }

    fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Panel::Sinks => Panel::Volume,
            Panel::Volume => Panel::AudioFx,
            Panel::AudioFx => Panel::AddButton,
            Panel::AddButton => Panel::Songs,
            Panel::Songs => Panel::Sinks,
        };
    }

    fn cycle_focus_back(&mut self) {
        self.focus = match self.focus {
            Panel::Sinks => Panel::Songs,
            Panel::Volume => Panel::Sinks,
            Panel::AudioFx => Panel::Volume,
            Panel::AddButton => Panel::AudioFx,
            Panel::Songs => Panel::AddButton,
        };
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
            _ => {}
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
            _ => {}
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
                }
            }
            Panel::AudioFx => {
                if self.selected_fx > 0 {
                    self.selected_fx -= 1;
                }
            }
            Panel::AddButton | Panel::Volume => {}
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
                }
            }
            Panel::AudioFx => {
                if self.selected_fx < 1 {
                    self.selected_fx += 1;
                }
            }
            Panel::AddButton | Panel::Volume => {}
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
            Panel::Sinks | Panel::Volume | Panel::AudioFx => {}
        }
    }

    fn delete_selected_song(&mut self) {
        if self.focus == Panel::Songs && !self.state.songs.is_empty() {
            self.send_command(ClientCommand::RemoveSong(self.state.selected_song));
        }
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
    std::process::Command::new(exe)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
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
    send_message(&mut stream, &ClientCommand::Quit)?;
    println!("Sent stop signal to daemon.");
    Ok(())
}
