use plentysound::audio::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;
use vosk::{Model, Recognizer};

// ── Manifest types ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Manifest {
    samples: Vec<SampleEntry>,
}

#[derive(Deserialize)]
struct SampleEntry {
    file: String,
    keywords: Vec<KeywordExpectation>,
}

#[derive(Deserialize, Clone)]
struct KeywordExpectation {
    word: String,
    expected: usize,
}

// ── Result tracking ──────────────────────────────────────────────────────────

struct VariantResult {
    strategy: String,
    recognition: String,
    counts: Vec<usize>,
    durations: Vec<std::time::Duration>,
}

struct KeywordResult {
    file: String,
    word: String,
    expected: usize,
    variants: Vec<VariantResult>,
}

// ── Strategy definition ─────────────────────────────────────────────────────

struct Strategy {
    name: &'static str,
    cooldown: usize,
}

const STRATEGIES: &[Strategy] = &[
    Strategy { name: "no-dedup", cooldown: 0 },
    Strategy { name: "consec", cooldown: 1 },
    Strategy { name: "gap-2", cooldown: 2 },
    Strategy { name: "gap-3", cooldown: 3 },
];

struct RecognitionVariant {
    name: &'static str,
    preprocess: bool,
    use_fuzzy: bool,
}

const RECOGNITION_VARIANTS: &[RecognitionVariant] = &[
    RecognitionVariant { name: "base", preprocess: false, use_fuzzy: false },
    RecognitionVariant { name: "enh", preprocess: true, use_fuzzy: true },
];

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

    let mut results: Vec<KeywordResult> = Vec::new();
    let mut sample_timings: Vec<(String, std::time::Duration)> = Vec::new();
    let total_start = Instant::now();

    let total_jobs = available.iter().map(|e| ROUNDS * STRATEGIES.len() * RECOGNITION_VARIANTS.len()).sum::<usize>();
    eprintln!("Spawning {total_jobs} jobs across {} sample(s)...", available.len());

    for entry in &available {
        let wav_path = samples_dir.join(&entry.file);
        let pcm = read_wav_i16(&wav_path);
        let keyword_words: Vec<&str> = entry.keywords.iter().map(|k| k.word.as_str()).collect();
        let chunks = chunk_audio(&pcm);

        let num_combos = STRATEGIES.len() * RECOGNITION_VARIANTS.len();

        // round_counts[keyword_idx][combo_idx] -> Vec<usize>
        let round_counts: Vec<Vec<Mutex<Vec<usize>>>> = entry
            .keywords
            .iter()
            .map(|_| (0..num_combos).map(|_| Mutex::new(Vec::new())).collect())
            .collect();

        // variant_durations[combo_idx] -> Vec<Duration> (one per round)
        let variant_durations: Vec<Mutex<Vec<std::time::Duration>>> =
            (0..num_combos).map(|_| Mutex::new(Vec::new())).collect();

        let sample_start = Instant::now();

        // Run all rounds x strategies x recognition variants in parallel
        std::thread::scope(|s| {
            for _round in 0..ROUNDS {
                for (si, strategy) in STRATEGIES.iter().enumerate() {
                    for (ri, recog) in RECOGNITION_VARIANTS.iter().enumerate() {
                        let combo_idx = si * RECOGNITION_VARIANTS.len() + ri;
                        let model = &model;
                        let chunks = &chunks;
                        let keyword_words = &keyword_words;
                        let keywords_meta = &entry.keywords;
                        let round_counts = &round_counts;
                        let variant_durations = &variant_durations;

                        s.spawn(move || {
                            let start = Instant::now();
                            let counts = run_variant(
                                model,
                                chunks,
                                keyword_words,
                                recog.preprocess,
                                recog.use_fuzzy,
                                strategy.cooldown,
                            );
                            let elapsed = start.elapsed();

                            variant_durations[combo_idx].lock().unwrap().push(elapsed);

                            for (ki, kw) in keywords_meta.iter().enumerate() {
                                let val = *counts.get(&kw.word).unwrap_or(&0);
                                round_counts[ki][combo_idx].lock().unwrap().push(val);
                            }
                        });
                    }
                }
            }
        });

        sample_timings.push((entry.file.clone(), sample_start.elapsed()));

        for (ki, kw) in entry.keywords.iter().enumerate() {
            let mut variants = Vec::new();
            for (si, strategy) in STRATEGIES.iter().enumerate() {
                for (ri, recog) in RECOGNITION_VARIANTS.iter().enumerate() {
                    let combo_idx = si * RECOGNITION_VARIANTS.len() + ri;
                    variants.push(VariantResult {
                        strategy: strategy.name.to_string(),
                        recognition: recog.name.to_string(),
                        counts: round_counts[ki][combo_idx].lock().unwrap().clone(),
                        durations: variant_durations[combo_idx].lock().unwrap().clone(),
                    });
                }
            }
            results.push(KeywordResult {
                file: entry.file.clone(),
                word: kw.word.clone(),
                expected: kw.expected,
                variants,
            });
        }
    }

    let total_elapsed = total_start.elapsed();
    print_table(&results, &sample_timings, total_elapsed);

    // No assertion — expected counts are used to compute accuracy in the table
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

// ── Generic variant runner ──────────────────────────────────────────────────

fn run_variant(
    model: &Model,
    chunks: &[Vec<i16>],
    keywords: &[&str],
    preprocess: bool,
    use_fuzzy: bool,
    cooldown: usize,
) -> HashMap<String, usize> {
    let grammar: Vec<&str> = keywords.iter().copied().chain(std::iter::once("[unk]")).collect();
    let mut recognizer = Recognizer::new_with_grammar(model, SAMPLE_RATE as f32, &grammar)
        .expect("Failed to create recognizer");

    let mut counts: HashMap<String, usize> = HashMap::new();
    // Per-keyword: last chunk index where it was detected
    let mut last_detected: HashMap<String, usize> = HashMap::new();

    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        let audio: Vec<i16>;
        let samples = if preprocess {
            audio = {
                let mut clean = highpass_filter(chunk);
                normalize(&mut clean);
                clean
            };
            &audio
        } else {
            chunk
        };

        recognizer.accept_waveform(samples);

        let text = recognizer
            .final_result()
            .single()
            .map(|r| r.text.to_string())
            .unwrap_or_default();

        let matched = if use_fuzzy {
            check_keywords_matched(&text, keywords)
        } else {
            check_keywords_exact(&text, keywords)
        };

        if let Some(keyword) = matched {
            // Dedup logic: only count if enough chunks have elapsed since last detection
            let should_count = if cooldown == 0 {
                // No dedup: always count
                true
            } else if let Some(&last_idx) = last_detected.get(&keyword) {
                // Count only if current - last >= cooldown + 1
                chunk_idx >= last_idx + cooldown + 1
            } else {
                // First detection of this keyword
                true
            };

            if should_count {
                *counts.entry(keyword.clone()).or_insert(0) += 1;
                last_detected.insert(keyword, chunk_idx);
            }
        }

        recognizer.reset();
    }
    counts
}

// ── Output table ─────────────────────────────────────────────────────────────

fn accuracy_pct(counts: &[usize], expected: usize) -> f64 {
    if expected == 0 {
        return 0.0;
    }
    let hits = counts.iter().filter(|&&c| c >= expected).count();
    hits as f64 / counts.len() as f64 * 100.0
}

fn rounds_str(counts: &[usize]) -> String {
    counts.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" ")
}

fn print_table(
    results: &[KeywordResult],
    sample_timings: &[(String, std::time::Duration)],
    total_elapsed: std::time::Duration,
) {
    if results.is_empty() {
        return;
    }

    let col_headers: Vec<String> = results[0]
        .variants
        .iter()
        .map(|v| format!("{}/{}", v.strategy, v.recognition))
        .collect();

    let col_width = 18;

    eprintln!();
    eprintln!("── Accuracy Benchmark ({ROUNDS} rounds) ──────────────────────────────────────────────────────────────────────────────────────────────────");

    let mut current_file = String::new();
    // Track variant durations for the timing row (take from first keyword per file)
    let mut pending_durations: Option<&[VariantResult]> = None;

    for (i, r) in results.iter().enumerate() {
        // Print timing row for previous file before switching
        if r.file != current_file {
            if let Some(variants) = pending_durations {
                eprint!(" {:<12} {:>3}", "avg time", "");
                for v in variants {
                    let avg_ms = if v.durations.is_empty() {
                        0.0
                    } else {
                        let total: f64 = v.durations.iter().map(|d| d.as_secs_f64() * 1000.0).sum();
                        total / v.durations.len() as f64
                    };
                    let timing_str = format!("{:.0}ms", avg_ms);
                    eprint!("  {:>width$}", timing_str, width = col_width);
                }
                eprintln!();
            }

            current_file = r.file.clone();
            let timing = sample_timings
                .iter()
                .find(|(f, _)| f == &current_file)
                .map(|(_, d)| format!("{:.2}s", d.as_secs_f64()))
                .unwrap_or_default();
            eprintln!(" {} ({})", current_file, timing);
            eprint!(" {:<12} {:>3}", "Keyword", "Exp");
            for h in &col_headers {
                eprint!("  {:>width$}", h, width = col_width);
            }
            eprintln!();
            pending_durations = Some(&r.variants);
        }

        eprint!(" {:<12} {:>3}", r.word, r.expected);
        for v in &r.variants {
            let pct = accuracy_pct(&v.counts, r.expected);
            let rs = rounds_str(&v.counts);
            eprint!("  {:>5.0}% [{:<width$}]", pct, rs, width = col_width - 9);
        }
        eprintln!();
    }

    // Print timing row for the last file
    if let Some(variants) = pending_durations {
        eprint!(" {:<12} {:>3}", "avg time", "");
        for v in variants {
            let avg_ms = if v.durations.is_empty() {
                0.0
            } else {
                let total: f64 = v.durations.iter().map(|d| d.as_secs_f64() * 1000.0).sum();
                total / v.durations.len() as f64
            };
            let timing_str = format!("{:.0}ms", avg_ms);
            eprint!("  {:>width$}", timing_str, width = col_width);
        }
        eprintln!();
    }

    eprintln!("────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────");
    eprintln!(" Total: {:.2}s", total_elapsed.as_secs_f64());
    eprintln!();
}
