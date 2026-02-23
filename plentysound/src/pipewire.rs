use anyhow::Result;
use pipewire::{
    context::Context,
    main_loop::MainLoop,
    properties::properties,
    spa::{
        param::{
            audio::{AudioFormat, AudioInfoRaw},
            ParamType,
        },
        pod::{serialize::PodSerializer, Object, Pod, Value},
        utils::SpaTypes,
    },
    stream::{Stream, StreamFlags},
};
use std::sync::mpsc::{Receiver, Sender};

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Output,
    Input,
}

#[derive(Debug, Clone)]
pub struct PwSink {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub kind: DeviceKind,
}

pub enum PwCommand {
    ListSinks,
    Play {
        sink_id: u32,
        kind: DeviceKind,
        node_name: String,
        samples: Vec<f32>,
        sample_rate: u32,
        channels: u32,
        volume: f32,
        comfort_noise: f32,
        eq_mid_boost: f32,
    },
}

#[derive(Debug)]
pub enum PwEvent {
    SinksUpdated(Vec<PwSink>),
    PlaybackFinished,
}

// ── PipeWire thread ──────────────────────────────────────────────────────────

pub fn spawn_pw_thread(
    cmd_rx: Receiver<PwCommand>,
    evt_tx: Sender<PwEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(e) = pw_thread_main(cmd_rx, evt_tx) {
            crate::log::log_error(&format!("PipeWire thread error: {e}"));
        }
    })
}

fn pw_thread_main(cmd_rx: Receiver<PwCommand>, evt_tx: Sender<PwEvent>) -> Result<()> {
    // Helper: do a fresh enumeration using a temporary PW connection
    fn enumerate_devices() -> Result<Vec<PwSink>> {
        let mainloop = MainLoop::new(None)?;
        let context = Context::new(&mainloop)?;
        let core = context.connect(None)?;
        let registry = core.get_registry()?;

        use std::collections::{HashMap, HashSet};

        struct RawSink {
            id: u32,
            name: String,
            description: String,
            kind: DeviceKind,
            client_id: Option<u32>,
        }

        // Store client globals so we can bind to them later
        type ClientGlobal = pipewire::registry::GlobalObject<pipewire::properties::Properties>;

        let client_globals = std::sync::Arc::new(std::sync::Mutex::new(Vec::<ClientGlobal>::new()));
        let client_globals_clone = client_globals.clone();
        let client_ids_needed = std::sync::Arc::new(std::sync::Mutex::new(HashSet::<u32>::new()));
        let client_ids_clone = client_ids_needed.clone();
        let raw_sinks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<RawSink>::new()));
        let raw_sinks_clone = raw_sinks.clone();

        // Pass 1: collect audio nodes and client globals
        let _reg_listener = registry
            .add_listener_local()
            .global(move |global| {
                if let Some(props) = global.props {
                    // Store client globals for later binding
                    if global.type_ == pipewire::types::ObjectType::Client {
                        client_globals_clone.lock().unwrap().push(ClientGlobal {
                            id: global.id,
                            permissions: global.permissions,
                            type_: global.type_.clone(),
                            version: global.version,
                            props: None,
                        });
                        return;
                    }

                    let media_class = props.get("media.class").unwrap_or("");
                    let kind = match media_class {
                        "Audio/Sink" => Some(DeviceKind::Output),
                        "Stream/Input/Audio" => Some(DeviceKind::Input),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        let id = global.id;
                        let name = props.get("node.name").unwrap_or("").to_string();
                        let client_id: Option<u32> = props
                            .get("client.id")
                            .and_then(|s| s.parse().ok());
                        let description = match kind {
                            DeviceKind::Input => {
                                if let Some(cid) = client_id {
                                    client_ids_clone.lock().unwrap().insert(cid);
                                }
                                if name.is_empty() { format!("Stream #{id}") } else { name.clone() }
                            }
                            DeviceKind::Output => {
                                props.get("node.description").unwrap_or(&name).to_string()
                            }
                        };
                        raw_sinks_clone.lock().unwrap().push(RawSink {
                            id, name, description, kind, client_id,
                        });
                    }
                }
            })
            .register();

        let pending = core.sync(0).expect("sync failed");
        let mainloop_weak = mainloop.downgrade();
        let _core_listener = core
            .add_listener_local()
            .done(move |id, seq| {
                if id == pipewire::core::PW_ID_CORE && seq == pending {
                    if let Some(ml) = mainloop_weak.upgrade() {
                        ml.quit();
                    }
                }
            })
            .register();

        mainloop.run();

        // Pass 2: bind to each needed client to get application.process.binary
        let clients_map = std::sync::Arc::new(std::sync::Mutex::new(HashMap::<u32, String>::new()));
        let needed = client_ids_needed.lock().unwrap().clone();
        let stored_globals = client_globals.lock().unwrap();

        // Keep bound proxies and listeners alive until roundtrip completes
        let mut _bound_clients = Vec::new();
        let mut _client_listeners = Vec::new();

        for global in stored_globals.iter().filter(|g| needed.contains(&g.id)) {
            let clients_map_clone = clients_map.clone();
            let cid_copy = global.id;
            match registry.bind::<pipewire::client::Client, _>(global) {
                Ok(client) => {
                    let listener = client
                        .add_listener_local()
                        .info(move |info| {
                            if let Some(props) = info.props() {
                                let binary = props
                                    .get("application.process.binary")
                                    .unwrap_or("");
                                if !binary.is_empty() {
                                    clients_map_clone
                                        .lock()
                                        .unwrap()
                                        .insert(cid_copy, binary.to_string());
                                }
                            }
                        })
                        .register();
                    _client_listeners.push(listener);
                    _bound_clients.push(client);
                }
                Err(_) => {}
            }
        }

        drop(stored_globals);

        // Second roundtrip to receive client info
        if !_bound_clients.is_empty() {
            let pending2 = core.sync(0).expect("sync failed");
            let mainloop_weak2 = mainloop.downgrade();
            let _core_listener2 = core
                .add_listener_local()
                .done(move |id, seq| {
                    if id == pipewire::core::PW_ID_CORE && seq == pending2 {
                        if let Some(ml) = mainloop_weak2.upgrade() {
                            ml.quit();
                        }
                    }
                })
                .register();

            mainloop.run();
        }

        // Enrich Input descriptions with the resolved app binary
        let cmap = clients_map.lock().unwrap();
        let result: Vec<PwSink> = raw_sinks
            .lock()
            .unwrap()
            .drain(..)
            .map(|raw| {
                let description = if raw.kind == DeviceKind::Input {
                    if let Some(binary) = raw.client_id.and_then(|cid| cmap.get(&cid)) {
                        if binary != &raw.description {
                            format!("{} ({})", raw.description, binary)
                        } else {
                            raw.description
                        }
                    } else {
                        raw.description
                    }
                } else {
                    raw.description
                };
                PwSink {
                    id: raw.id,
                    name: raw.name,
                    description,
                    kind: raw.kind,
                }
            })
            .collect();
        Ok(result)
    }

    // Initial enumeration
    let devices = enumerate_devices()?;
    let _ = evt_tx.send(PwEvent::SinksUpdated(devices));

    // Process commands
    for cmd in cmd_rx {
        match cmd {
            PwCommand::ListSinks => {
                let devices = enumerate_devices().unwrap_or_default();
                let _ = evt_tx.send(PwEvent::SinksUpdated(devices));
            }
            PwCommand::Play {
                sink_id,
                kind,
                node_name: _,
                samples,
                sample_rate,
                channels,
                volume,
                comfort_noise,
                eq_mid_boost,
            } => {
                let evt_tx_play = evt_tx.clone();
                std::thread::spawn(move || {
                    let result = match kind {
                        DeviceKind::Output => play_audio_threaded(sink_id, samples, sample_rate, channels, volume, comfort_noise, eq_mid_boost),
                        DeviceKind::Input => play_to_input_stream(sink_id, samples, sample_rate, channels, volume, comfort_noise, eq_mid_boost),
                    };
                    if let Err(e) = result {
                        crate::log::log_error(&format!("Playback error: {e}"));
                    }
                    let _ = evt_tx_play.send(PwEvent::PlaybackFinished);
                });
            }
        }
    }

    Ok(())
}

// Peaking EQ biquad coefficients (Audio EQ Cookbook)
// center_freq = 1000 Hz, Q = 1.0, gain derived from eq_mid_boost
fn compute_biquad(sample_rate: f32, boost: f32) -> [f32; 5] {
    let gain_db = 20.0 * boost.log10();
    let a_val = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * 1000.0 / sample_rate;
    let sin_w0 = w0.sin();
    let cos_w0 = w0.cos();
    let alpha = sin_w0 / 2.0; // Q = 1.0
    let b0 = 1.0 + alpha * a_val;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha * a_val;
    let a0 = 1.0 + alpha / a_val;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha / a_val;
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

// Simple xorshift64 PRNG for noise generation
fn next_noise(state: &std::sync::atomic::AtomicU64) -> f32 {
    use std::sync::atomic::Ordering;
    let mut s = state.load(Ordering::Relaxed);
    if s == 0 { s = 0xDEADBEEFCAFE; }
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    state.store(s, Ordering::Relaxed);
    (s as i64 as f32) / (i64::MAX as f32)
}

fn play_audio_threaded(
    sink_id: u32,
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u32,
    volume: f32,
    comfort_noise: f32,
    eq_mid_boost: f32,
) -> Result<()> {
    let mainloop = MainLoop::new(None)?;
    let context = Context::new(&mainloop)?;
    let core = context.connect(None)?;

    let props = properties! {
        "media.type"     => "Audio",
        "media.category" => "Playback",
        "media.role"     => "Music",
    };

    let stream = Stream::new(&core, "plentysound-playback", props)?;

    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(sample_rate);
    audio_info.set_channels(channels);

    let obj = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let pod_value = Value::Object(obj);
    let (pod_bytes, _) = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod_value)
        .map_err(|e| anyhow::anyhow!("pod serialize error: {:?}", e))?;
    let pod_bytes = pod_bytes.into_inner();
    let param = Pod::from_bytes(&pod_bytes).unwrap();

    stream.connect(
        pipewire::spa::utils::Direction::Output,
        Some(sink_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut [param],
    )?;

    let total_samples = samples.len();
    let samples = std::sync::Arc::new(samples);
    let samples_clone = samples.clone();
    let offset = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let offset_clone = offset.clone();
    let mainloop_weak = mainloop.downgrade();

    let apply_eq = eq_mid_boost != 1.0 && eq_mid_boost > 0.0;
    let biquad = compute_biquad(sample_rate as f32, if apply_eq { eq_mid_boost } else { 1.0 });
    let rng_state = std::sync::atomic::AtomicU64::new(0xDEADBEEFCAFE);
    // Biquad state: [x1, x2, y1, y2] per channel (max 8 channels)
    let mut eq_state = [[0.0f32; 4]; 8];

    let _listener = stream
        .add_local_listener()
        .process(move |stream, _: &mut ()| {
            if let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let data = &mut datas[0];
                if let Some(slice) = data.data() {
                    let out_samples = slice.len() / std::mem::size_of::<f32>();
                    let mut pos = offset_clone.lock().unwrap();

                    let remaining = samples_clone.len() - *pos;
                    let to_write = out_samples.min(remaining);

                    let out_f32: &mut [f32] = unsafe {
                        std::slice::from_raw_parts_mut(
                            slice.as_mut_ptr() as *mut f32,
                            out_samples,
                        )
                    };
                    for i in 0..to_write {
                        let mut sample = samples_clone[*pos + i] * volume;

                        // Apply biquad EQ
                        if apply_eq {
                            let ch = i % channels as usize;
                            if ch < 8 {
                                let st = &mut eq_state[ch];
                                let y = biquad[0] * sample + biquad[1] * st[0] + biquad[2] * st[1]
                                    - biquad[3] * st[2] - biquad[4] * st[3];
                                st[1] = st[0];
                                st[0] = sample;
                                st[3] = st[2];
                                st[2] = y;
                                sample = y;
                            }
                        }

                        // Add comfort noise
                        out_f32[i] = sample + next_noise(&rng_state) * comfort_noise;
                    }

                    for i in to_write..out_samples {
                        out_f32[i] = next_noise(&rng_state) * comfort_noise;
                    }

                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = std::mem::size_of::<f32>() as i32 * channels as i32;
                    *chunk.size_mut() = (to_write * std::mem::size_of::<f32>()) as u32;

                    *pos += to_write;

                    if *pos >= total_samples {
                        if let Some(ml) = mainloop_weak.upgrade() {
                            ml.quit();
                        }
                    }
                }
            }
        })
        .register()?;

    mainloop.run();

    Ok(())
}

fn play_to_input_stream(
    target_id: u32,
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u32,
    volume: f32,
    comfort_noise: f32,
    eq_mid_boost: f32,
) -> Result<()> {
    // Same approach as play_audio_threaded, but using node.target property
    // to tell WirePlumber to route our playback into the target capture stream
    let mainloop = MainLoop::new(None)?;
    let context = Context::new(&mainloop)?;
    let core = context.connect(None)?;

    let target_str = target_id.to_string();
    let props = properties! {
        "media.type"     => "Audio",
        "media.category" => "Playback",
        "media.role"     => "Music",
        "node.name"      => "plentysound-inject",
        "node.target"    => target_str.as_str(),
    };

    let stream = Stream::new(&core, "plentysound-inject", props)?;

    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(sample_rate);
    audio_info.set_channels(channels);

    let obj = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let pod_value = Value::Object(obj);
    let (pod_bytes, _) = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod_value)
        .map_err(|e| anyhow::anyhow!("pod serialize error: {:?}", e))?;
    let pod_bytes = pod_bytes.into_inner();
    let param = Pod::from_bytes(&pod_bytes).unwrap();

    stream.connect(
        pipewire::spa::utils::Direction::Output,
        Some(target_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut [param],
    )?;

    let total_samples = samples.len();
    let samples = std::sync::Arc::new(samples);
    let samples_clone = samples.clone();
    let offset = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let offset_clone = offset.clone();
    let mainloop_weak = mainloop.downgrade();

    let apply_eq = eq_mid_boost != 1.0 && eq_mid_boost > 0.0;
    let biquad = compute_biquad(sample_rate as f32, if apply_eq { eq_mid_boost } else { 1.0 });
    let rng_state = std::sync::atomic::AtomicU64::new(0xCAFEBABE1234);
    let mut eq_state = [[0.0f32; 4]; 8];

    let _listener = stream
        .add_local_listener()
        .process(move |stream, _: &mut ()| {
            if let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let data = &mut datas[0];
                if let Some(slice) = data.data() {
                    let out_samples = slice.len() / std::mem::size_of::<f32>();
                    let mut pos = offset_clone.lock().unwrap();

                    let remaining = samples_clone.len() - *pos;
                    let to_write = out_samples.min(remaining);

                    let out_f32: &mut [f32] = unsafe {
                        std::slice::from_raw_parts_mut(
                            slice.as_mut_ptr() as *mut f32,
                            out_samples,
                        )
                    };
                    for i in 0..to_write {
                        let mut sample = samples_clone[*pos + i] * volume;

                        if apply_eq {
                            let ch = i % channels as usize;
                            if ch < 8 {
                                let st = &mut eq_state[ch];
                                let y = biquad[0] * sample + biquad[1] * st[0] + biquad[2] * st[1]
                                    - biquad[3] * st[2] - biquad[4] * st[3];
                                st[1] = st[0];
                                st[0] = sample;
                                st[3] = st[2];
                                st[2] = y;
                                sample = y;
                            }
                        }

                        out_f32[i] = sample + next_noise(&rng_state) * comfort_noise;
                    }

                    for i in to_write..out_samples {
                        out_f32[i] = next_noise(&rng_state) * comfort_noise;
                    }

                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = std::mem::size_of::<f32>() as i32 * channels as i32;
                    *chunk.size_mut() = (to_write * std::mem::size_of::<f32>()) as u32;

                    *pos += to_write;

                    if *pos >= total_samples {
                        if let Some(ml) = mainloop_weak.upgrade() {
                            ml.quit();
                        }
                    }
                }
            }
        })
        .register()?;

    mainloop.run();

    Ok(())
}
