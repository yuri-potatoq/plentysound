use biquad::{Biquad, Coefficients, DirectForm2Transposed, Hertz, Type, Q_BUTTERWORTH_F64};
use strsim::jaro_winkler;

// ── Config ───────────────────────────────────────────────────────────────────

pub const SAMPLE_RATE: f64 = 16_000.0;
pub const CHANNELS: u32 = 1;
pub const KEYWORDS: &[&str] = &["olá", "ola", "oi", "lucas"];

pub const CHUNK_SECS: f64 = 1.5;
pub const OVERLAP_SECS: f64 = 0.5;
pub const CHUNK_SAMPLES: usize = (SAMPLE_RATE * CHUNK_SECS) as usize; // 24000
pub const OVERLAP_SAMPLES: usize = (SAMPLE_RATE * OVERLAP_SECS) as usize; // 8000

pub const FUZZY_THRESHOLD: f64 = 0.85; // Jaro-Winkler similarity threshold

// Path to your downloaded Vosk model directory
pub const MODEL_PATH: &str = "/home/potatoq/Downloads/vosk-model-small-pt-0.3";

// ── Audio preprocessing ──────────────────────────────────────────────────────

/// High-pass filter (80 Hz Butterworth) to remove rumble/hum/DC offset
pub fn highpass_filter(samples: &[i16]) -> Vec<i16> {
    let coeffs = Coefficients::<f64>::from_params(
        Type::HighPass,
        Hertz::<f64>::from_hz(SAMPLE_RATE).unwrap(),
        Hertz::<f64>::from_hz(80.0).unwrap(),
        Q_BUTTERWORTH_F64,
    )
    .unwrap();
    let mut filter = DirectForm2Transposed::<f64>::new(coeffs);

    samples
        .iter()
        .map(|&s| {
            let y = filter.run(s as f64);
            y.clamp(i16::MIN as f64, i16::MAX as f64) as i16
        })
        .collect()
}

/// Normalize audio to a target RMS level for consistent volume
pub fn normalize(samples: &mut [i16]) {
    let target_rms: f64 = 3000.0;
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    if rms < 100.0 {
        return; // silence — don't amplify noise
    }
    let gain = (target_rms / rms).min(10.0);
    for s in samples.iter_mut() {
        let v = (*s as f64 * gain).clamp(i16::MIN as f64, i16::MAX as f64);
        *s = v as i16;
    }
}

// ── Keyword matching ─────────────────────────────────────────────────────────

/// Check text against the given keywords with exact + fuzzy matching.
/// Returns the first matched keyword, if any.
pub fn check_keywords_matched(text: &str, keywords: &[&str]) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let text_lower = text.to_lowercase();
    for &keyword in keywords {
        if text_lower.contains(keyword) || fuzzy_match(&text_lower, keyword) {
            return Some(keyword.to_string());
        }
    }
    None
}

/// Check text against keywords using exact `contains()` only (no fuzzy).
/// Returns the first matched keyword, if any.
pub fn check_keywords_exact(text: &str, keywords: &[&str]) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let text_lower = text.to_lowercase();
    for &keyword in keywords {
        if text_lower.contains(keyword) {
            return Some(keyword.to_string());
        }
    }
    None
}

/// Fuzzy match using Jaro-Winkler similarity (good for short strings/typos)
pub fn fuzzy_match(text: &str, keyword: &str) -> bool {
    if keyword.chars().count() < 3 {
        return false;
    }
    text.split_whitespace()
        .any(|word| jaro_winkler(word, keyword) >= FUZZY_THRESHOLD)
}
