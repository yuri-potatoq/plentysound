use plentysound::audio::*;
use serde::Deserialize;
use std::path::Path;
use vosk::{Model, Recognizer};

// ── Manifest types ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Manifest {
    samples: Vec<SampleEntry>,
}

#[derive(Deserialize)]
struct SampleEntry {
    file: String,
    keywords: Vec<String>,
}

// ── Result tracking ──────────────────────────────────────────────────────────

struct SampleResult {
    file: String,
    baseline_hits: usize,
    enhanced_hits: usize,
    rounds: usize,
}

// ── Test ─────────────────────────────────────────────────────────────────────

const ROUNDS: usize = 5;

#[test]
fn accuracy_benchmark() {
    let samples_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples");
    let manifest_path = samples_dir.join("manifest.toml");

    if !manifest_path.exists() {
        eprintln!("⚠  Skipping accuracy benchmark: manifest not found at {}", manifest_path.display());
        return;
    }

    let manifest_text = std::fs::read_to_string(&manifest_path)
        .expect("Failed to read manifest.toml");
    let manifest: Manifest = toml::from_str(&manifest_text)
        .expect("Failed to parse manifest.toml");

    // Filter to samples that actually exist on disk
    let available: Vec<&SampleEntry> = manifest
        .samples
        .iter()
        .filter(|s| samples_dir.join(&s.file).exists())
        .collect();

    if available.is_empty() {
        eprintln!("⚠  Skipping accuracy benchmark: no WAV files found in {}", samples_dir.display());
        return;
    }

    // Load Vosk model
    let model = Model::new(MODEL_PATH).expect("Failed to load Vosk model");

    let mut results: Vec<SampleResult> = Vec::new();

    for entry in &available {
        let wav_path = samples_dir.join(&entry.file);
        let pcm = read_wav_i16(&wav_path);
        let keywords: Vec<&str> = entry.keywords.iter().map(|s| s.as_str()).collect();

        let chunks = chunk_audio(&pcm);

        let mut baseline_hits = 0usize;
        let mut enhanced_hits = 0usize;

        for _round in 0..ROUNDS {
            // ── Baseline: raw audio, exact match only ────────────────────
            if run_baseline(&model, &chunks, &keywords) {
                baseline_hits += 1;
            }

            // ── Enhanced: highpass + normalize + fuzzy ────────────────────
            if run_enhanced(&model, &chunks, &keywords) {
                enhanced_hits += 1;
            }
        }

        results.push(SampleResult {
            file: entry.file.clone(),
            baseline_hits,
            enhanced_hits,
            rounds: ROUNDS,
        });
    }

    print_table(&results);
}

// ── WAV reading ──────────────────────────────────────────────────────────────

fn read_wav_i16(path: &Path) -> Vec<i16> {
    let reader = hound::WavReader::open(path)
        .unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e));

    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "Expected mono WAV, got {} channels", spec.channels);
    assert_eq!(spec.sample_rate, SAMPLE_RATE as u32,
        "Expected {}Hz WAV, got {}Hz", SAMPLE_RATE as u32, spec.sample_rate);

    match spec.sample_format {
        hound::SampleFormat::Int => {
            reader.into_samples::<i16>()
                .map(|s| s.expect("Failed to read sample"))
                .collect()
        }
        hound::SampleFormat::Float => {
            reader.into_samples::<f32>()
                .map(|s| {
                    let v = s.expect("Failed to read sample");
                    (v * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16
                })
                .collect()
        }
    }
}

// ── Chunking ─────────────────────────────────────────────────────────────────

fn chunk_audio(pcm: &[i16]) -> Vec<Vec<i16>> {
    let mut chunks = Vec::new();
    let advance = CHUNK_SAMPLES - OVERLAP_SAMPLES;
    let mut offset = 0;

    while offset + CHUNK_SAMPLES <= pcm.len() {
        chunks.push(pcm[offset..offset + CHUNK_SAMPLES].to_vec());
        offset += advance;
    }

    // Include trailing partial chunk if any samples remain
    if offset < pcm.len() {
        let mut tail = pcm[offset..].to_vec();
        // Pad with silence to fill a full chunk
        tail.resize(CHUNK_SAMPLES, 0);
        chunks.push(tail);
    }

    chunks
}

// ── Variant runners ──────────────────────────────────────────────────────────

fn run_baseline(model: &Model, chunks: &[Vec<i16>], keywords: &[&str]) -> bool {
    let grammar: Vec<&str> = keywords.iter().copied().chain(std::iter::once("[unk]")).collect();
    let mut recognizer = Recognizer::new_with_grammar(model, SAMPLE_RATE as f32, &grammar)
        .expect("Failed to create recognizer");

    for chunk in chunks {
        // Feed raw audio (no preprocessing)
        recognizer.accept_waveform(chunk);

        let text = recognizer
            .final_result()
            .single()
            .map(|r| r.text.to_string())
            .unwrap_or_default();

        if check_keywords_exact(&text, keywords).is_some() {
            return true;
        }
        recognizer.reset();
    }
    false
}

fn run_enhanced(model: &Model, chunks: &[Vec<i16>], keywords: &[&str]) -> bool {
    let grammar: Vec<&str> = keywords.iter().copied().chain(std::iter::once("[unk]")).collect();
    let mut recognizer = Recognizer::new_with_grammar(model, SAMPLE_RATE as f32, &grammar)
        .expect("Failed to create recognizer");

    for chunk in chunks {
        // Preprocess: high-pass filter + normalize
        let mut clean = highpass_filter(chunk);
        normalize(&mut clean);

        recognizer.accept_waveform(&clean);

        let text = recognizer
            .final_result()
            .single()
            .map(|r| r.text.to_string())
            .unwrap_or_default();

        // Enhanced uses exact + fuzzy matching
        if check_keywords_matched(&text, keywords).is_some() {
            return true;
        }
        recognizer.reset();
    }
    false
}

// ── Output table ─────────────────────────────────────────────────────────────

fn print_table(results: &[SampleResult]) {
    let rounds = results.first().map(|r| r.rounds).unwrap_or(ROUNDS);

    println!();
    println!("── Accuracy Benchmark ({rounds} rounds) ──────────────────────────────");
    println!("{:<20}│ {:>10} │ {:>10}", "Sample", "Baseline", "Enhanced");

    let mut total_baseline = 0usize;
    let mut total_enhanced = 0usize;
    let mut total_rounds = 0usize;

    for r in results {
        let bp = if r.rounds > 0 { r.baseline_hits * 100 / r.rounds } else { 0 };
        let ep = if r.rounds > 0 { r.enhanced_hits * 100 / r.rounds } else { 0 };
        println!(
            "{:<20}│ {:>2}/{} {:>3}% │ {:>2}/{} {:>3}%",
            r.file, r.baseline_hits, r.rounds, bp, r.enhanced_hits, r.rounds, ep,
        );
        total_baseline += r.baseline_hits;
        total_enhanced += r.enhanced_hits;
        total_rounds += r.rounds;
    }

    let total_bp = if total_rounds > 0 { total_baseline * 100 / total_rounds } else { 0 };
    let total_ep = if total_rounds > 0 { total_enhanced * 100 / total_rounds } else { 0 };
    println!("────────────────────┼────────────┼────────────");
    println!(
        "{:<20}│ {:>2}/{} {:>3}%│ {:>2}/{} {:>3}%",
        "Overall", total_baseline, total_rounds, total_bp, total_enhanced, total_rounds, total_ep,
    );
    println!();
}
