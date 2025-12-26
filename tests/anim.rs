use std::collections::BTreeMap;
use termtosvg::anim;
use termtosvg::anim::CharacterCell;
use termtosvg::term::TimedFrame;

fn cell(text: &str, fg: &str, bg: &str) -> CharacterCell {
    CharacterCell::with_colors(text, fg, bg)
}

#[test]
fn render_line_bg_colors_groups_rectangles() {
    let mut line = BTreeMap::new();
    line.insert(0, cell("A", "red", "red"));
    line.insert(1, cell("B", "red", "red"));
    line.insert(3, cell("C", "red", "blue"));

    let rects = anim::render_line_bg_colors(&line, 0, anim::CELL_HEIGHT, anim::CELL_WIDTH);
    assert_eq!(rects.len(), 2);
    assert_eq!(rects[0].attributes.get("width").unwrap(), "16");
}

#[test]
fn render_characters_groups_text_with_same_style() {
    let mut line = BTreeMap::new();
    line.insert(0, cell("A", "red", "background"));
    line.insert(1, cell("B", "red", "background"));
    line.insert(5, cell("C", "blue", "background"));

    let group = anim::render_characters(&line, anim::CELL_WIDTH);
    // expect consecutive cells with same style to merge into one text node
    let count = group
        .children
        .iter()
        .filter(|n| matches!(n, xmltree::XMLNode::Element(e) if e.name == "text"))
        .count();
    assert_eq!(count, 2);
}

#[test]
fn render_animation_and_validate() {
    let frames = vec![TimedFrame {
        time: 0,
        duration: 1000,
        buffer: BTreeMap::new(),
    }];
    let output = tempfile::NamedTempFile::new().unwrap();
    anim::render_animation(frames.clone(), (80, 24), output.path(), b"<svg></svg>")
        .unwrap();
    let bytes = std::fs::read(output.path()).unwrap();
    anim::validate_svg(bytes.as_slice()).unwrap();
}
