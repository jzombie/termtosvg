use termtosvg::asciicast::{self, AsciiCastV2Event, AsciiCastV2Header, AsciiCastV2Theme};

#[test]
fn theme_validation_rejects_invalid_colors() {
    assert!(
        AsciiCastV2Theme::new(
            "red",
            "#000000",
            "#000000:#111111:#222222:#333333:#444444:#555555:#666666:#777777"
        )
        .is_err()
    );
}

#[test]
fn header_roundtrip() {
    let header = AsciiCastV2Header::new(2, 80, 24, None, Some(1.234)).unwrap();
    let line = header.to_json_line().unwrap();
    let parsed = AsciiCastV2Header::from_json_line(&line).unwrap();
    assert_eq!(header, parsed);
}

#[test]
fn event_roundtrip() {
    let event = AsciiCastV2Event::new(0.0, "o", "data", None).unwrap();
    let line = event.to_json_line().unwrap();
    let parsed = AsciiCastV2Event::from_json_line(&line).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn read_v1_records_falls_back() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = r#"{
    "version": 1,
    "width": 80,
    "height": 24,
    "duration": 2,
    "stdout": [[0.5, "hi"], [0.5, "!"]]
}"#;
    std::fs::write(tmp.path(), content).unwrap();
    let records = asciicast::read_records(tmp.path()).unwrap();
    assert!(
        records
            .iter()
            .any(|r| matches!(r, asciicast::AsciiCastV2Record::Header(_)))
    );
}
