use std::path::PathBuf;

const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "flac", "ogg", "opus"];

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

pub struct FileBrowser {
    pub current_dir: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
}

impl FileBrowser {
    pub fn new() -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"));
        let mut fb = FileBrowser {
            current_dir: home,
            entries: Vec::new(),
            selected: 0,
        };
        fb.refresh();
        fb
    }

    pub fn refresh(&mut self) {
        let mut dirs = Vec::new();
        let mut files = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(&self.current_dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                if name.starts_with('.') {
                    continue;
                }

                if path.is_dir() {
                    dirs.push(Entry {
                        name,
                        path,
                        is_dir: true,
                    });
                } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                        files.push(Entry {
                            name,
                            path,
                            is_dir: false,
                        });
                    }
                }
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        self.entries = dirs;
        self.entries.extend(files);
        self.selected = 0;
    }

    pub fn navigate_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.refresh();
        }
    }

    pub fn navigate_into(&mut self) {
        if let Some(entry) = self.entries.get(self.selected) {
            if entry.is_dir {
                self.current_dir = entry.path.clone();
                self.refresh();
            }
        }
    }

    /// Returns Some(path) if a file was selected, None if navigated into dir
    pub fn select(&mut self) -> Option<PathBuf> {
        if let Some(entry) = self.entries.get(self.selected) {
            if entry.is_dir {
                self.navigate_into();
                None
            } else {
                Some(entry.path.clone())
            }
        } else {
            None
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.entries.is_empty() && self.selected < self.entries.len() - 1 {
            self.selected += 1;
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}
