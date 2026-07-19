//! Offline STT smoke test: run a synthesized-speech fixture (tests/fixtures/
//! stt_fixture.wav, 16 kHz mono, generated with macOS `say`) through the real
//! transcribe-cpp whisper engine and assert the words come back.
//!
//! Skips gracefully (passes with a notice) when the whisper model file is not
//! on disk, so CI runners without the ~900 MB model still pass. Point
//! VAPORLY_STT_TEST_MODEL at a .gguf to use a different model.

use std::path::PathBuf;

fn default_model_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VAPORLY_STT_TEST_MODEL")
        .or_else(|_| std::env::var("FLOWLOCAL_STT_TEST_MODEL"))
    {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(
        "Library/Application Support/computer.vaporly/models/whisper-large-v3-turbo-Q8_0.gguf",
    ))
}

#[test]
fn synthesized_speech_transcribes() {
    let Some(model_path) = default_model_path() else {
        eprintln!("SKIP: no HOME and no VAPORLY_STT_TEST_MODEL set");
        return;
    };
    if !model_path.exists() {
        eprintln!(
            "SKIP: STT model not present at {} (set VAPORLY_STT_TEST_MODEL to override)",
            model_path.display()
        );
        return;
    }

    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stt_fixture.wav");
    let mut reader = hound::WavReader::open(&wav).expect("open fixture wav");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
    assert_eq!(spec.channels, 1, "fixture must be mono");
    let audio: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.expect("read sample") as f32 / 32768.0)
        .collect();
    assert!(audio.len() > 16_000, "fixture should be at least 1 second");

    // CPU backend: deterministic everywhere, and faster than the paravirtual
    // GPU in VMs. A ~3.5 s clip decodes in a few seconds.
    let options = transcribe_cpp::ModelOptions {
        backend: transcribe_cpp::Backend::Cpu,
        gpu_device: 0, // 0 = auto / first matching device
    };
    let model =
        transcribe_cpp::Model::load_with(&model_path, &options).expect("load whisper model");
    let mut session = model.session().expect("create session");
    let text = session
        .run(
            &audio,
            &transcribe_cpp::RunOptions {
                timestamps: transcribe_cpp::TimestampKind::None,
                ..Default::default()
            },
        )
        .expect("transcription runs")
        .text;

    eprintln!("STT transcript: {text}");
    let lower = text.to_lowercase();
    assert!(lower.contains("hello"), "expected 'hello' in: {text}");
    assert!(
        lower.contains("test") || lower.contains("recording") || lower.contains("dictation"),
        "expected the fixture phrase in: {text}"
    );
}

/// Pseudo-streaming's single engine-level assumption: a transcribe-cpp session
/// can be `run` repeatedly on a growing window with stable output. Feeds the
/// fixture in two partial windows then the full clip, the way
/// run_pseudo_stream re-decodes, and asserts each decode succeeds and the
/// final one still contains the fixture phrase.
#[test]
fn repeated_window_decode_is_stable() {
    let Some(model_path) = default_model_path() else {
        eprintln!("SKIP: no HOME and no VAPORLY_STT_TEST_MODEL set");
        return;
    };
    if !model_path.exists() {
        eprintln!("SKIP: STT model not present at {}", model_path.display());
        return;
    }

    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stt_fixture.wav");
    let mut reader = hound::WavReader::open(&wav).expect("open fixture wav");
    let audio: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.expect("read sample") as f32 / 32768.0)
        .collect();

    let options = transcribe_cpp::ModelOptions {
        backend: transcribe_cpp::Backend::Cpu,
        gpu_device: 0,
    };
    let model =
        transcribe_cpp::Model::load_with(&model_path, &options).expect("load whisper model");
    let mut session = model.session().expect("create session");

    let run = |session: &mut transcribe_cpp::Session, window: &[f32]| -> String {
        session
            .run(
                window,
                &transcribe_cpp::RunOptions {
                    timestamps: transcribe_cpp::TimestampKind::None,
                    ..Default::default()
                },
            )
            .expect("partial decode runs")
            .text
    };

    let one_sec = 16_000usize;
    let t1 = run(&mut session, &audio[..one_sec.min(audio.len())]);
    let t2 = run(&mut session, &audio[..(2 * one_sec).min(audio.len())]);
    let t3 = run(&mut session, &audio);
    eprintln!("window decodes: 1s={t1:?} 2s={t2:?} full={t3:?}");
    assert!(
        t3.to_lowercase().contains("hello"),
        "final window decode lost the phrase: {t3}"
    );
}
