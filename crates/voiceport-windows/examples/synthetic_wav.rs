//! Explicit test artifact generation; never used by the default shell.
fn main() {
    let path = std::env::args_os()
        .nth(1)
        .expect("owned output path required");
    let file = std::fs::File::create_new(path).unwrap();
    let mut wav = voiceport_windows::wav::Wav::new(file).unwrap();
    wav.append(&[-32768, 0, 32767]).unwrap();
    wav.finish().unwrap();
}
