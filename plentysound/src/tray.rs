use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct PlentySoundTray {
    shutdown: Arc<AtomicBool>,
    now_playing: Arc<Mutex<Option<String>>>,
}

impl ksni::Tray for PlentySoundTray {
    fn title(&self) -> String {
        "plentysound".to_string()
    }

    fn icon_name(&self) -> String {
        "audio-volume-high".to_string()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let np_label = match self.now_playing.lock().unwrap().as_ref() {
            Some(name) => format!("Now Playing: {}", name),
            None => "Not playing".to_string(),
        };

        vec![
            ksni::MenuItem::Standard(ksni::menu::StandardItem {
                label: np_label,
                enabled: false,
                ..Default::default()
            }),
            ksni::MenuItem::Separator,
            ksni::MenuItem::Standard(ksni::menu::StandardItem {
                label: "Quit".to_string(),
                activate: Box::new(|tray: &mut Self| {
                    tray.shutdown.store(true, Ordering::SeqCst);
                }),
                ..Default::default()
            }),
        ]
    }
}

pub fn spawn_tray(shutdown: Arc<AtomicBool>, now_playing: Arc<Mutex<Option<String>>>) {
    std::thread::spawn(move || {
        let tray = PlentySoundTray {
            shutdown,
            now_playing,
        };
        let service = ksni::TrayService::new(tray);
        service.run();
    });
}
