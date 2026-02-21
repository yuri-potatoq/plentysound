use pipewire::{
    context::Context,
    main_loop::MainLoop,
    properties::properties,
    spa::{
        param::{audio::{AudioFormat, AudioInfoRaw}, ParamType},
        pod::{serialize::PodSerializer, Object, Pod, Value},
        utils::SpaTypes,
    },
    stream::{Stream, StreamFlags},
};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use vosk::{Model, Recognizer};

use plentysound::audio::*;

// ── Event ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KeywordEvent {
    pub keyword: String,
    pub text:    String,
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    println!("Loading Vosk model from {MODEL_PATH}...");
    let model = Model::new(MODEL_PATH).expect("Failed to load Vosk model");
    // Restrict recognition to only our keywords — uses far less memory
    let grammar: Vec<&str> = KEYWORDS.iter().copied().chain(std::iter::once("[unk]")).collect();
    let recognizer = Arc::new(Mutex::new(
        Recognizer::new_with_grammar(&model, SAMPLE_RATE as f32, &grammar)
            .expect("Failed to create recognizer"),
    ));

    let (tx, mut rx) = broadcast::channel::<KeywordEvent>(64);

    // ── Event consumer (your actions go here) ────────────────────────────────
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            println!(
                "🎯 Keyword '{}' detected in: \"{}\"",
                event.keyword, event.text
            );
            // TODO: trigger your action here
        }
    });

    // ── PipeWire stream (blocking, runs on its own thread) ───────────────────
    let tx_pw = tx.clone();
    let recognizer_pw = recognizer.clone();

    std::thread::spawn(move || {
        run_pipewire(tx_pw, recognizer_pw).expect("PipeWire error");
    });

    println!("Listening for keywords: {:?}", KEYWORDS);
    println!("Speak into your audio interface (e.g. Discord output monitor)...");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    println!("Shutting down.");
    Ok(())
}

// ── PipeWire capture loop ─────────────────────────────────────────────────────

fn run_pipewire(
    tx: broadcast::Sender<KeywordEvent>,
    recognizer: Arc<Mutex<Recognizer>>,
) -> anyhow::Result<()> {
    let mainloop = MainLoop::new(None)?;
    let context = Context::new(&mainloop)?;
    let core = context.connect(None)?;

    // Build stream properties
    // target.object.name lets you pick a specific PipeWire source.
    // Leave it unset to capture the default source, or set it to a specific
    // node name (find with: pw-dump | grep -A5 '"name"' | grep -i discord)
    let props = properties! {
        "media.type"     => "Audio",
        "media.category" => "Capture",
        "media.role"     => "Communication",
        "target.object.name" => "alsa_input.usb-HP__Inc_HyperX_QuadCast_4111-00.iec958-stereo",
    };

    let stream = Stream::new(&core, "keyword-detector", props)?;

    // Audio format: mono, 16kHz, signed 16-bit — what Vosk expects
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::S16LE);
    audio_info.set_rate(SAMPLE_RATE as u32);
    audio_info.set_channels(CHANNELS);

    let obj = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let pod_value = Value::Object(obj);
    let (pod_bytes, _) =
        PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod_value)
            .map_err(|e| anyhow::anyhow!("pod serialize error: {:?}", e))?;
    let pod_bytes = pod_bytes.into_inner();
    let param = Pod::from_bytes(&pod_bytes).unwrap();

    stream.connect(
        pipewire::spa::utils::Direction::Input,
        None, // auto-select node
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut [param],
    )?;

    // Accumulation buffer for fixed-size chunks
    let mut ring: Vec<i16> = Vec::with_capacity(CHUNK_SAMPLES + 8192);

    // Register process callback — called for every audio buffer
    let _listener = stream
        .add_local_listener()
        .process(move |stream, _: &mut ()| {
            if let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let data = &mut datas[0];
                let chunk = data.chunk();
                let offset = chunk.offset() as usize;
                let size   = chunk.size() as usize;

                if let Some(slice) = data.data() {
                    let pcm_bytes = &slice[offset..offset + size];
                    let samples: Vec<i16> = pcm_bytes
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect();

                    ring.extend_from_slice(&samples);

                    // Process every time we have a full chunk
                    while ring.len() >= CHUNK_SAMPLES {
                        let chunk_data: Vec<i16> = ring[..CHUNK_SAMPLES].to_vec();
                        // Slide forward by (chunk - overlap), keeping the overlap tail
                        let advance = CHUNK_SAMPLES - OVERLAP_SAMPLES;
                        ring.drain(..advance);

                        process_audio(&chunk_data, &tx, &recognizer);
                    }
                }
            }
        })
        .register()?;

    mainloop.run();
    Ok(())
}

// ── Audio → Vosk → keyword match ─────────────────────────────────────────────

fn process_audio(
    samples: &[i16],
    tx: &broadcast::Sender<KeywordEvent>,
    recognizer: &Arc<Mutex<Recognizer>>,
) {
    // Preprocess: high-pass filter + normalize
    let mut clean = highpass_filter(samples);
    normalize(&mut clean);

    let mut rec = recognizer.lock().unwrap();

    // Feed the chunk and check partial result first (lower latency)
    let state = rec.accept_waveform(&clean);

    let text = match state {
        vosk::DecodingState::Running => {
            let partial = rec.partial_result().partial.to_string();
            if check_keywords(&partial, tx) {
                rec.reset();
                return;
            }
            // Not found in partial — finalize
            rec.final_result()
                .single()
                .map(|r| r.text.to_string())
                .unwrap_or_default()
        }
        vosk::DecodingState::Finalized => rec
            .final_result()
            .single()
            .map(|r| r.text.to_string())
            .unwrap_or_default(),
        vosk::DecodingState::Failed => {
            rec.reset();
            return;
        }
    };

    check_keywords(&text, tx);
    rec.reset();
}

/// Check text against keywords with fuzzy matching.
/// Returns true if a keyword was found.
fn check_keywords(text: &str, tx: &broadcast::Sender<KeywordEvent>) -> bool {
    if let Some(keyword) = check_keywords_matched(text, KEYWORDS) {
        let _ = tx.send(KeywordEvent {
            keyword,
            text: text.to_string(),
        });
        true
    } else {
        false
    }
}
