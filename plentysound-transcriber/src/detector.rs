use crate::audio::{
    check_keywords_matched, check_keywords_exact, highpass_filter, normalize,
    CHUNK_SAMPLES, MIN_TAIL_SAMPLES, OVERLAP_SAMPLES, SAMPLE_RATE,
};
use anyhow::{Context, Result};
use biquad::Biquad;
use pipewire::{
    context::Context as PwContext,
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
use std::cell::RefCell;
use std::sync::mpsc;
use vosk::{Model, Recognizer};

/// PipeWire will likely deliver at this rate regardless of what we request.
const PW_SAMPLE_RATE: u32 = 48_000;

/// Number of channels PipeWire will likely deliver (stereo).
const PW_CHANNELS: u32 = 2;

/// Vosk expects this rate (matches SAMPLE_RATE from audio.rs = 16000).
const VOSK_SAMPLE_RATE: u32 = SAMPLE_RATE as u32;

/// Cooldown: ignore same keyword if detected again within this many seconds.
const DEDUP_COOLDOWN_SECS: f64 = 3.0;

/// Mix interleaved samples down to mono, then downsample with a low-pass
/// anti-aliasing filter to avoid spectral aliasing that corrupts speech.
fn stereo_to_mono_and_downsample(samples: &[i16], channels: u32, src_rate: u32, dst_rate: u32) -> Vec<i16> {
    let ch = channels as usize;
    let mono: Vec<i16> = samples
        .chunks_exact(ch)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            (sum / ch as i32) as i16
        })
        .collect();

    let ratio = (src_rate / dst_rate) as usize;
    if ratio <= 1 {
        return mono;
    }

    // Anti-aliasing low-pass filter at dst Nyquist (dst_rate/2) before decimation.
    // This prevents frequencies above Nyquist from aliasing into the speech band.
    let nyquist = dst_rate as f64 / 2.0;
    // Cut slightly below Nyquist to give the filter room to roll off
    let cutoff = nyquist * 0.9;
    let coeffs = biquad::Coefficients::<f64>::from_params(
        biquad::Type::LowPass,
        biquad::Hertz::<f64>::from_hz(src_rate as f64).unwrap(),
        biquad::Hertz::<f64>::from_hz(cutoff).unwrap(),
        biquad::Q_BUTTERWORTH_F64,
    )
    .unwrap();
    let mut filter = biquad::DirectForm2Transposed::<f64>::new(coeffs);

    mono.iter()
        .map(|&s| {
            let y = filter.run(s as f64);
            y.clamp(i16::MIN as f64, i16::MAX as f64) as i16
        })
        .step_by(ratio)
        .collect()
}

/// Run the word detector loop.
///
/// Captures audio from the given PipeWire node, preprocesses it (highpass
/// filter + normalization), runs Vosk recognition with overlapping chunks,
/// and calls `on_match` for each deduplicated keyword detection using
/// exact + fuzzy (Jaro-Winkler) matching.
///
/// Returns when `stop_rx` receives a message or the channel is closed.
pub fn run_detector(
    model_path: &str,
    keywords: &[String],
    pw_target_node: u32,
    stop_rx: mpsc::Receiver<()>,
    on_match: impl Fn(String) + Send + 'static,
    log: impl Fn(&str) + 'static,
) -> Result<()> {
    let log = std::sync::Arc::new(log);
    log(&format!("Loading Vosk model from: {}", model_path));
    let model = Model::new(model_path).context("Failed to load Vosk model")?;
    log("Vosk model loaded");

    // Deduplicate keywords for grammar
    let mut unique_keywords: Vec<String> = Vec::new();
    for kw in keywords {
        let lower = kw.to_lowercase();
        if !unique_keywords.contains(&lower) {
            unique_keywords.push(lower);
        }
    }

    // Build grammar: unique keywords + unknown token
    let grammar: Vec<&str> = unique_keywords
        .iter()
        .map(|s| s.as_str())
        .chain(std::iter::once("[unk]"))
        .collect();
    log(&format!("Creating recognizer with grammar: {:?}", grammar));
    let recognizer = Recognizer::new_with_grammar(&model, VOSK_SAMPLE_RATE as f32, &grammar)
        .context("Failed to create Vosk recognizer")?;

    // Set up PipeWire capture
    let mainloop = MainLoop::new(None)?;
    let context = PwContext::new(&mainloop)?;
    let core = context.connect(None)?;

    let target_str = pw_target_node.to_string();
    log(&format!(
        "PipeWire capture: node={}, {}Hz {}ch -> {}Hz mono, chunk={} overlap={} samples",
        pw_target_node, PW_SAMPLE_RATE, PW_CHANNELS, VOSK_SAMPLE_RATE, CHUNK_SAMPLES, OVERLAP_SAMPLES
    ));
    let stream = Stream::new(
        &core,
        "plentysound-detector",
        properties! {
            "media.type"     => "Audio",
            "media.category" => "Capture",
            "media.role"     => "Communication",
            "node.target"    => target_str.as_str(),
        },
    )?;

    // Raw audio buffer shared between PipeWire callback and timer.
    let audio_buf: std::sync::Arc<std::sync::Mutex<Vec<i16>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let audio_buf_pw = audio_buf.clone();

    let _listener = stream
        .add_local_listener()
        .process(move |stream, _: &mut ()| {
            if let Some(mut buf) = stream.dequeue_buffer() {
                let datas = buf.datas_mut();
                if let Some(data) = datas.first_mut() {
                    let offset = data.chunk().offset() as usize;
                    let size = data.chunk().size() as usize;
                    if let Some(slice) = data.data() {
                        let end = (offset + size).min(slice.len());
                        let valid = if size > 0 && end > offset {
                            &slice[offset..end]
                        } else {
                            slice
                        };
                        let samples: Vec<i16> = valid
                            .chunks_exact(2)
                            .map(|c| i16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        audio_buf_pw.lock().unwrap().extend_from_slice(&samples);
                    }
                }
            }
        })
        .register()?;

    // Request S16LE stereo at PW native rate
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::S16LE);
    audio_info.set_rate(PW_SAMPLE_RATE);
    audio_info.set_channels(PW_CHANNELS);

    let obj = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let pod_value = Value::Object(obj);
    let (pod_bytes, _) = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod_value)
        .map_err(|e| anyhow::anyhow!("Failed to serialize audio params: {:?}", e))?;
    let pod_bytes = pod_bytes.into_inner();
    let param = Pod::from_bytes(&pod_bytes)
        .ok_or_else(|| anyhow::anyhow!("Failed to create Pod from bytes"))?;

    stream.connect(
        pipewire::spa::utils::Direction::Input,
        Some(pw_target_node),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut [param],
    )?;
    log("PipeWire capture stream connected");

    // Stop flag
    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag_watcher = stop_flag.clone();
    std::thread::spawn(move || {
        let _ = stop_rx.recv();
        stop_flag_watcher.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    // Mutable state in RefCell (timer callback is Fn, not FnMut)
    let recognizer = RefCell::new(recognizer);
    // Mono 16kHz buffer that accumulates converted samples for chunked processing
    let mono_buf: RefCell<Vec<i16>> = RefCell::new(Vec::new());
    let chunk_count: RefCell<u64> = RefCell::new(0);
    // Dedup: last keyword + timestamp
    let last_match: RefCell<Option<(String, std::time::Instant)>> = RefCell::new(None);

    let advance = CHUNK_SAMPLES - OVERLAP_SAMPLES;

    // Timer callback: convert audio, preprocess in chunks, feed to Vosk
    let timer = mainloop.loop_().add_timer({
        let audio_buf = audio_buf.clone();
        let stop_flag = stop_flag.clone();
        let mainloop_weak = mainloop.downgrade();
        let log = log.clone();
        let keyword_strs_owned: Vec<String> = unique_keywords.clone();
        move |_| {
            if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                if let Some(ml) = mainloop_weak.upgrade() {
                    ml.quit();
                }
                return;
            }

            let keyword_refs: Vec<&str> = keyword_strs_owned.iter().map(|s| s.as_str()).collect();

            // Drain raw PW audio and convert to 16kHz mono
            let new_mono = {
                let mut buf = audio_buf.lock().unwrap();
                if buf.is_empty() {
                    return;
                }
                let raw: Vec<i16> = buf.drain(..).collect();
                stereo_to_mono_and_downsample(&raw, PW_CHANNELS, PW_SAMPLE_RATE, VOSK_SAMPLE_RATE)
            };

            // Append to mono accumulation buffer
            let mut mbuf = mono_buf.borrow_mut();
            mbuf.extend_from_slice(&new_mono);

            // Process overlapping chunks: CHUNK_SAMPLES (1.5s) with OVERLAP_SAMPLES (0.75s)
            while mbuf.len() >= CHUNK_SAMPLES {
                let chunk: Vec<i16> = mbuf[..CHUNK_SAMPLES].to_vec();
                mbuf.drain(..advance);

                // Audio preprocessing: highpass filter + RMS normalization
                let mut processed = highpass_filter(&chunk);
                normalize(&mut processed);

                // Compute RMS for logging
                let current_count = {
                    let mut cc = chunk_count.borrow_mut();
                    *cc += 1;
                    *cc
                };

                if current_count % 30 == 0 {
                    let sum_sq: f64 = processed.iter().map(|&s| (s as f64) * (s as f64)).sum();
                    let rms = (sum_sq / processed.len().max(1) as f64).sqrt();
                    log(&format!(
                        "Chunk {}: {} samples, RMS={:.0}, buf_remaining={}",
                        current_count, processed.len(), rms, mbuf.len()
                    ));
                }

                // Feed preprocessed chunk to Vosk
                let mut rec = recognizer.borrow_mut();
                let state = rec.accept_waveform(&processed);

                if matches!(state, vosk::DecodingState::Finalized) {
                    let text = rec
                        .final_result()
                        .single()
                        .map(|r| r.text.to_string())
                        .unwrap_or_default();

                    if !text.is_empty() && text != "[unk]" {
                        log(&format!("Vosk final: \"{}\"", text));

                        // Use full matching (exact + fuzzy) on final results
                        if let Some(keyword) = check_keywords_matched(&text, &keyword_refs) {
                            try_emit_match(
                                &keyword, &last_match, &on_match, log.as_ref(),
                                "final",
                            );
                        }
                    } else if current_count % 30 == 0 {
                        log(&format!("Vosk final (silence): \"{}\"", text));
                    }
                } else {
                    // Check partial results for early detection
                    let partial = rec.partial_result().partial.to_string();

                    if !partial.is_empty() && partial != "[unk]" {
                        if current_count % 15 == 0 {
                            log(&format!("Vosk partial: \"{}\"", partial));
                        }

                        // Use exact-only matching on partials (avoids false positives
                        // from rapidly changing partial text)
                        if let Some(keyword) = check_keywords_exact(&partial, &keyword_refs) {
                            try_emit_match(
                                &keyword, &last_match, &on_match, log.as_ref(),
                                "partial",
                            );
                        }
                    }
                }
                drop(rec);
            }

            // Process tail: if there are leftover samples that haven't formed
            // a full chunk, pad with silence and feed to Vosk so words spoken
            // near the end of a burst aren't lost.
            if !mbuf.is_empty() && mbuf.len() >= MIN_TAIL_SAMPLES && mbuf.len() < CHUNK_SAMPLES {
                let mut tail = mbuf.to_vec();
                tail.resize(CHUNK_SAMPLES, 0);

                let mut processed = highpass_filter(&tail);
                normalize(&mut processed);

                let mut rec = recognizer.borrow_mut();
                let state = rec.accept_waveform(&processed);

                if matches!(state, vosk::DecodingState::Finalized) {
                    let text = rec
                        .final_result()
                        .single()
                        .map(|r| r.text.to_string())
                        .unwrap_or_default();

                    if !text.is_empty() && text != "[unk]" {
                        log(&format!("Vosk final (tail): \"{}\"", text));
                        if let Some(keyword) = check_keywords_matched(&text, &keyword_refs) {
                            try_emit_match(
                                &keyword, &last_match, &on_match, log.as_ref(),
                                "tail",
                            );
                        }
                    }
                }
                drop(rec);
                // Don't drain — let it accumulate into a full chunk next time
            }
        }
    });

    // Fire every 100ms
    timer.update_timer(
        Some(std::time::Duration::from_millis(100)),
        Some(std::time::Duration::from_millis(100)),
    );

    log("Detector mainloop starting");
    mainloop.run();
    log("Detector mainloop exited");

    drop(timer);
    drop(_listener);
    drop(stream);

    Ok(())
}

/// Try to emit a keyword match, applying time-based deduplication.
fn try_emit_match(
    keyword: &str,
    last_match: &RefCell<Option<(String, std::time::Instant)>>,
    on_match: &dyn Fn(String),
    log: &dyn Fn(&str),
    source: &str,
) {
    let now = std::time::Instant::now();
    let is_dup = {
        let last = last_match.borrow();
        if let Some((ref last_kw, ref last_time)) = *last {
            last_kw == keyword && now.duration_since(*last_time).as_secs_f64() < DEDUP_COOLDOWN_SECS
        } else {
            false
        }
    };

    log(&format!(
        "Keyword matched ({}): \"{}\" (dup={})",
        source, keyword, is_dup
    ));

    if !is_dup {
        on_match(keyword.to_string());
        *last_match.borrow_mut() = Some((keyword.to_string(), now));
    }
}

/// Check if the Vosk library is available/loadable.
pub fn check_vosk_available() -> Result<()> {
    vosk::set_log_level(vosk::LogLevel::Error);
    Ok(())
}
