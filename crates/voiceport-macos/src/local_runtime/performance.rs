//! Per-file recognizer creation and decode timings for locally supplied fixtures.
use super::*;
use std::time::Instant;

pub(crate) fn performance_file(
    id: &str,
    directory: &Path,
    runtime: &Path,
    sample: &Path,
    language: &str,
    runs: u32,
    peak_footprint: impl Fn() -> u64,
) {
    assert!((1..=31).contains(&runs));
    assert!(directory.is_absolute() && runtime.is_absolute() && sample.is_absolute());
    let definition = crate::model_library::MODELS
        .iter()
        .find(|item| item.id == id)
        .expect("catalog model ID");
    let model = Model::discover(directory.to_path_buf(), runtime, definition.kind).unwrap();
    let cancel = CancellationToken::new();
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
    for iteration in 1..=runs {
        let start = Instant::now();
        let mut session = Session::new(&model, language, &cancel).unwrap();
        let opened = start.elapsed();
        let mut first = None;
        let mut partial = |text: String| {
            if first.is_none() && !text.trim().is_empty() {
                first = Some(ms(start.elapsed()));
            }
        };
        crate::audio::read_pcm(sample, |packet| session.push(packet, &cancel, &mut partial))
            .unwrap();
        let audio_read = start.elapsed();
        let text = session.finish(&cancel, &mut partial).unwrap();
        let final_text = start.elapsed();
        println!(
            "{}",
            serde_json::json!({
                "scene": "local_file_recognizer", "iteration": iteration,
                "open_ms": ms(opened),
                "read_and_feed_ms": ms(audio_read - opened),
                "flush_and_final_ms": ms(final_text - audio_read),
                "first_nonempty_partial_ms": first,
                "final_ms": ms(final_text),
                "peak_phys_footprint_bytes": peak_footprint(),
                "text": text,
            })
        );
    }
}
