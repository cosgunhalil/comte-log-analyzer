//! Live tail must produce exactly what a full parse produces, however the
//! file's bytes are split into chunks (Unity's parser holds entries back).

use std::path::Path;

use comte_core::{LogParser, TailDocument, parse_file};
use comte_unity::{EditorParser, PlayerParser};

fn check(name: &str, new_parser: fn() -> Box<dyn LogParser>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let bytes = std::fs::read(&path).unwrap();
    let whole = parse_file(&path, new_parser().as_mut()).unwrap();
    for chunk_size in [1, 2, 17, 333, 4096, bytes.len()] {
        let mut tail = TailDocument::new(new_parser());
        for chunk in bytes.chunks(chunk_size) {
            tail.push_bytes(chunk);
            let _ = tail.snapshot(); // snapshots between chunks must not disturb parsing
        }
        let snapshot = tail.snapshot();
        assert_eq!(
            snapshot.entries(),
            whole.entries(),
            "{name}, chunk size {chunk_size}"
        );
        assert_eq!(
            snapshot.sections(),
            whole.sections(),
            "{name}, chunk size {chunk_size}"
        );
        assert_eq!(snapshot.facts(), whole.facts());
        assert_eq!(snapshot.metrics(), whole.metrics());
    }
}

#[test]
fn editor_log_tails_like_a_full_parse() {
    check("editor_session.log", || Box::new(EditorParser::new()));
}

#[test]
fn player_log_tails_like_a_full_parse() {
    check("player_session.log", || Box::new(PlayerParser::new()));
}
