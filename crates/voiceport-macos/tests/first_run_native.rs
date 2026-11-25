#[cfg(target_os = "macos")]
fn main() {
    voiceport_macos::first_run_field_fixture().expect("isolated AppKit first-run check");
    println!("first-run AppKit fixture: caret, single write, moved-caret refusal, repeat passed");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
