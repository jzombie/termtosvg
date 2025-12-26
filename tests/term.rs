use std::collections::BTreeMap;

use termtosvg::anim::CharacterCell;
use termtosvg::asciicast::{AsciiCastV2Event, AsciiCastV2Header, AsciiCastV2Record};
use termtosvg::term;

fn header() -> AsciiCastV2Record {
    AsciiCastV2Record::Header(AsciiCastV2Header::new(2, 80, 24, None, None).unwrap())
}

fn visible_text(line: &BTreeMap<usize, CharacterCell>) -> String {
    fn is_cursor(cell: &CharacterCell) -> bool {
        cell.text == " " && cell.color == "background" && cell.background_color == "foreground"
    }

    let mut text = String::new();
    for cell in line.values() {
        if is_cursor(cell) {
            continue;
        }
        text.push_str(&cell.text);
    }
    text
}

#[test]
fn group_by_time_respects_min_duration() {
    let events = vec![
        AsciiCastV2Event::new(0.0, "o", "1", None).unwrap(),
        AsciiCastV2Event::new(0.005, "o", "2", None).unwrap(),
        AsciiCastV2Event::new(0.008, "o", "3", None).unwrap(),
    ];

    let grouped = term::_group_by_time(events, 5, Some(10), 1000);
    assert_eq!(grouped.len(), 2);
    assert_eq!(grouped[0].event_data, "1");
    assert_eq!(grouped[1].event_data, "23");
}

#[test]
fn timed_frames_emits_cursor_and_content() {
    let records = vec![
        header(),
        AsciiCastV2Record::Event(AsciiCastV2Event::new(0.0, "o", "a\r\nb", None).unwrap()),
    ];
    let (_geometry, mut frames) = term::timed_frames(records, 1, None, 1000);
    let first = frames.next().unwrap();
    assert!(first.buffer.get(&0).unwrap().get(&0).is_some());
    let has_cursor = first
        .buffer
        .values()
        .any(|row| row.values().any(|c| c.text == " "));
    assert!(has_cursor);
}

#[test]
fn zero_width_characters_are_preserved() {
    // sleuth emoji + variation selector + zero width joiner + 'a'
    let text = format!("e{}\u{fe0f}\u{200d}a", '\u{1f575}');
    let records = vec![
        header(),
        AsciiCastV2Record::Event(AsciiCastV2Event::new(0.0, "o", &text, None).unwrap()),
    ];
    let (_geometry, mut frames) = term::timed_frames(records, 1, None, 1000);
    let frame = frames.next().unwrap();
    let line = frame.buffer.get(&0).unwrap();
    let concatenated: String = line.values().map(|c| c.text.clone()).collect();
    assert!(concatenated.contains('a'));
}

#[test]
fn get_terminal_size_returns_default_for_invalid_fd() {
    let (cols, rows) = term::get_terminal_size(-1);
    assert!(cols >= 1 && rows >= 1);
}

#[test]
fn timed_frames_handles_delete_chars() {
    let seq = "abcd\x1b[2D\x1b[P\x1b[10G";
    let records = vec![
        header(),
        AsciiCastV2Record::Event(AsciiCastV2Event::new(0.0, "o", seq, None).unwrap()),
    ];
    let (_geometry, mut frames) = term::timed_frames(records, 1, None, 1000);
    let frame = frames.next().unwrap();
    let line = frame.buffer.get(&0).unwrap();
    assert_eq!(visible_text(line), "abd");
}

#[test]
fn timed_frames_handles_insert_blank_chars() {
    let seq = "abcd\x1b[2D\x1b[@Z\x1b[10G";
    let records = vec![
        header(),
        AsciiCastV2Record::Event(AsciiCastV2Event::new(0.0, "o", seq, None).unwrap()),
    ];
    let (_geometry, mut frames) = term::timed_frames(records, 1, None, 1000);
    let frame = frames.next().unwrap();
    let line = frame.buffer.get(&0).unwrap();
    assert_eq!(visible_text(line), "abZcd");
}

#[test]
fn timed_frames_handles_erase_chars() {
    let seq = "abcd\x1b[3D\x1b[2X\x1b[10G";
    let records = vec![
        header(),
        AsciiCastV2Record::Event(AsciiCastV2Event::new(0.0, "o", seq, None).unwrap()),
    ];
    let (_geometry, mut frames) = term::timed_frames(records, 1, None, 1000);
    let frame = frames.next().unwrap();
    let line = frame.buffer.get(&0).unwrap();
    assert_eq!(visible_text(line), "ad");
    assert!(line.get(&1).is_none());
    assert!(line.get(&2).is_none());
}
