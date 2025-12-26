use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use unicode_width::UnicodeWidthStr;
use xmltree::{Element, XMLNode};

use crate::term::TimedFrame;

pub const CELL_WIDTH: u16 = 8;
pub const CELL_HEIGHT: u16 = 17;

static BG_RECT: Lazy<Element> = Lazy::new(|| {
    let mut rect = Element::new("rect");
    rect.attributes.insert("class".into(), "background".into());
    rect.attributes.insert("height".into(), "100%".into());
    rect.attributes.insert("width".into(), "100%".into());
    rect.attributes.insert("x".into(), "0".into());
    rect.attributes.insert("y".into(), "0".into());
    rect
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterCell {
    pub text: String,
    pub color: String,
    pub background_color: String,
    pub bold: bool,
    pub italics: bool,
    pub underscore: bool,
    pub strikethrough: bool,
}

impl CharacterCell {
    pub fn new<T: Into<String>>(text: T) -> Self {
        CharacterCell {
            text: text.into(),
            color: "foreground".into(),
            background_color: "background".into(),
            bold: false,
            italics: false,
            underscore: false,
            strikethrough: false,
        }
    }

    pub fn with_colors<T: Into<String>>(text: T, color: &str, background_color: &str) -> Self {
        CharacterCell {
            color: color.into(),
            background_color: background_color.into(),
            ..CharacterCell::new(text)
        }
    }
}

#[derive(Clone, Copy)]
#[derive(PartialEq, Eq)]
pub struct ConsecutiveKey<'a> {
    pub color: &'a str,
    pub bold: bool,
    pub italics: bool,
    pub underscore: bool,
    pub strikethrough: bool,
}

fn make_text_tag(column: usize, attr: &CharacterCell, text: &str, cell_width: u16) -> Element {
    let mut text_elem = Element::new("text");
    text_elem
        .attributes
        .insert("x".into(), (column as u32 * cell_width as u32).to_string());
    text_elem.attributes.insert(
        "textLength".into(),
        (UnicodeWidthStr::width(text) as u32 * cell_width as u32).to_string(),
    );

    if attr.bold {
        text_elem.attributes.insert("font-weight".into(), "bold".into());
    }
    if attr.italics {
        text_elem.attributes.insert("font-style".into(), "italic".into());
    }
    let mut decoration = Vec::new();
    if attr.underscore {
        decoration.push("underline");
    }
    if attr.strikethrough {
        decoration.push("line-through");
    }
    if !decoration.is_empty() {
        text_elem
            .attributes
            .insert("text-decoration".into(), decoration.join(" "));
    }

    if attr.color.starts_with('#') {
        text_elem.attributes.insert("fill".into(), attr.color.clone());
    } else {
        text_elem.attributes.insert("class".into(), attr.color.clone());
    }

    text_elem.children.push(XMLNode::Text(text.to_string()));
    text_elem
}

fn make_rect_tag(column: usize, length: usize, height: usize, cell_width: u16, cell_height: u16, background_color: &str) -> Element {
    let mut rect = Element::new("rect");
    rect.attributes.insert("x".into(), (column as u32 * cell_width as u32).to_string());
    rect.attributes.insert("y".into(), height.to_string());
    rect.attributes.insert("width".into(), (length as u32 * cell_width as u32).to_string());
    rect.attributes.insert("height".into(), cell_height.to_string());

    if background_color.starts_with('#') {
        rect.attributes.insert("fill".into(), background_color.into());
    } else {
        rect.attributes.insert("class".into(), background_color.into());
    }

    rect
}

pub fn render_animation<I: IntoIterator<Item = TimedFrame>, P: AsRef<Path>>(frames: I, geometry: (u16, u16), filename: P, template: &[u8]) -> Result<()> {
    let mut root = render_preparation(geometry, template)?;
    let (_width, height) = geometry;
    let mut screen_view = Element::new("g");
    screen_view.attributes.insert("id".into(), "screen_view".into());

    let mut definitions: BTreeMap<String, Element> = BTreeMap::new();
    let mut timings: BTreeMap<u64, i32> = BTreeMap::new();
    let mut animation_duration = 0_u64;
    for (frame_idx, frame) in frames.into_iter().enumerate() {
        let offset = frame_idx as i32 * (height as i32 + 1) * CELL_HEIGHT as i32;
        let (group, defs) = render_timed_frame(offset, &frame.buffer, CELL_HEIGHT, CELL_WIDTH, &definitions);
        definitions.extend(defs);
        animation_duration = frame.time + frame.duration;
        timings.insert(frame.time, -offset);
        screen_view.children.push(XMLNode::Element(group));
    }

    // attach definitions
    let mut defs = Element::new("defs");
    for def in definitions.values() {
        defs.children.push(XMLNode::Element(def.clone()));
    }
    root.children.push(XMLNode::Element(defs));

    root.children.push(XMLNode::Element(screen_view));
    embed_css(&mut root, Some(&timings), Some(animation_duration))?;

    let mut file = File::create(filename)?;
    root.write(&mut file)?;
    // ensure not empty and simple validation
    let serialized_root = serialize_element(&root);
    validate_svg_bytes(serialized_root.as_bytes())?;
    Ok(())
}

pub fn render_still_frames<I: IntoIterator<Item = TimedFrame>, P: AsRef<Path>>(frames: I, geometry: (u16, u16), directory: P, template: &[u8]) -> Result<()> {
    std::fs::create_dir_all(directory.as_ref())?;
    let root = render_preparation(geometry, template)?;
    for (idx, frame) in frames.into_iter().enumerate() {
        let mut frame_root = root.clone();
        let (group, defs) = render_timed_frame(0, &frame.buffer, CELL_HEIGHT, CELL_WIDTH, &BTreeMap::new());

        let mut defs_elem = Element::new("defs");
        for def in defs.values() {
            defs_elem.children.push(XMLNode::Element(def.clone()));
        }

        frame_root.children.push(XMLNode::Element(defs_elem));
        frame_root.children.push(XMLNode::Element(group));
        embed_css(&mut frame_root, None, None)?;

        let filename = directory
            .as_ref()
            .join(format!("termtosvg_{:05}.svg", idx));
        let mut file = File::create(filename)?;
        frame_root.write(&mut file)?;
    }
    Ok(())
}

pub fn render_preparation(geometry: (u16, u16), template: &[u8]) -> Result<Element> {
    let mut root = resize_template(template, geometry)?;
    // clear previous screen content and ensure bg rect exists
    if let Some(screen) = root.get_mut_child("svg") {
        screen.children.clear();
        screen.children.push(XMLNode::Element(BG_RECT.clone()));
    }
    Ok(root)
}

pub fn render_timed_frame(
    offset: i32,
    buffer: &BTreeMap<usize, BTreeMap<usize, CharacterCell>>,
    cell_height: u16,
    cell_width: u16,
    definitions: &BTreeMap<String, Element>,
) -> (Element, BTreeMap<String, Element>) {
    let mut frame_group = Element::new("g");
    let mut new_definitions: BTreeMap<String, Element> = BTreeMap::new();

    for (row, line) in buffer.iter() {
        if line.is_empty() {
            continue;
        }
        let (mut tags, defs) = render_line(offset, *row, line, cell_height, cell_width, definitions, &new_definitions);
        frame_group.children.append(&mut tags);
        new_definitions.extend(defs);
    }

    (frame_group, new_definitions)
}

fn render_line(
    offset: i32,
    row: usize,
    row_data: &BTreeMap<usize, CharacterCell>,
    cell_height: u16,
    cell_width: u16,
    definitions: &BTreeMap<String, Element>,
    local_defs: &BTreeMap<String, Element>,
) -> (Vec<XMLNode>, BTreeMap<String, Element>) {
    let mut nodes = Vec::new();

    // backgrounds
    for rect in render_line_bg_colors(row_data, (offset + row as i32 * cell_height as i32) as usize, cell_height, cell_width) {
        nodes.push(XMLNode::Element(rect));
    }

    // text group
    let text_group = render_characters(row_data, cell_width);
    let serialized = serialize_element(&text_group);
    let mut new_defs = BTreeMap::new();
    let mut group_id = None;
    if let Some(existing) = definitions.get(&serialized).or_else(|| local_defs.get(&serialized)) {
        group_id = existing.attributes.get("id").cloned();
    }
    if group_id.is_none() {
        let id = format!("g{}", definitions.len() + local_defs.len() + 1);
        let mut tg = text_group.clone();
        tg.attributes.insert("id".into(), id.clone());
        new_defs.insert(serialized.clone(), tg);
        group_id = Some(id);
    }
    if let Some(id) = group_id {
        let mut use_tag = Element::new("use");
        use_tag
            .attributes
            .insert("href".into(), format!("#{id}"));
        use_tag
            .attributes
            .insert("y".into(), (offset + row as i32 * cell_height as i32).to_string());
        nodes.push(XMLNode::Element(use_tag));
    }

    (nodes, new_defs)
}

pub fn render_line_bg_colors(row: &BTreeMap<usize, CharacterCell>, height: usize, cell_height: u16, cell_width: u16) -> Vec<Element> {
    let mut rects = Vec::new();
    let iter: Vec<(usize, &CharacterCell)> = row
        .iter()
        .filter(|(_, c)| c.background_color != "background")
        .map(|(col, cell)| (*col, cell))
        .collect();
    if iter.is_empty() {
        return rects;
    }

    let mut start_col = iter[0].0;
    let mut current_color = iter[0].1.background_color.as_str();
    let mut accumulated_width = UnicodeWidthStr::width(iter[0].1.text.as_str());
    let mut last_col = iter[0].0;

    let flush = |start: usize, width: usize, color: &str, rects: &mut Vec<Element>| {
        if width > 0 {
            rects.push(make_rect_tag(start, width, height, cell_width, cell_height, color));
        }
    };

    for (col, cell) in iter.into_iter().skip(1) {
        let contiguous = col == last_col + 1;
        if contiguous && cell.background_color == current_color {
            accumulated_width += UnicodeWidthStr::width(cell.text.as_str());
        } else {
            flush(start_col, accumulated_width, current_color, &mut rects);
            start_col = col;
            current_color = cell.background_color.as_str();
            accumulated_width = UnicodeWidthStr::width(cell.text.as_str());
        }
        last_col = col;
    }

    flush(start_col, accumulated_width, current_color, &mut rects);
    rects
}

pub fn render_characters(row: &BTreeMap<usize, CharacterCell>, cell_width: u16) -> Element {
    let mut text_group = Element::new("g");
    if row.is_empty() {
        return text_group;
    }

    let mut sorted: Vec<(usize, &CharacterCell)> = row.iter().map(|(c, cell)| (*c, cell)).collect();
    sorted.sort_by_key(|(c, _)| *c);

    let mut current_key: Option<ConsecutiveKey<'_>> = None;
    let mut current_text = String::new();
    let mut current_col = 0_usize;
    let mut last_col: Option<usize> = None;

    for (col, cell) in sorted.into_iter() {
        let key = ConsecutiveKey {
            color: &cell.color,
            bold: cell.bold,
            italics: cell.italics,
            underscore: cell.underscore,
            strikethrough: cell.strikethrough,
        };
        let contiguous = last_col.map(|lc| col == lc + 1).unwrap_or(true);
        let same_group = contiguous && current_key.map(|ck| ck == key).unwrap_or(false);

        if same_group {
            current_text.push_str(&cell.text);
        } else {
            if let Some(prev_key) = current_key {
                let grouped_cell = cell_from_key(prev_key, &current_text);
                let elem = make_text_tag(current_col, &grouped_cell, &current_text, cell_width);
                text_group.children.push(XMLNode::Element(elem));
            }
            current_key = Some(key);
            current_col = col;
            current_text = cell.text.clone();
        }
        last_col = Some(col);
    }

    if let Some(prev_key) = current_key {
        let grouped_cell = cell_from_key(prev_key, &current_text);
        let elem = make_text_tag(current_col, &grouped_cell, &current_text, cell_width);
        text_group.children.push(XMLNode::Element(elem));
    }

    text_group
}

fn cell_from_key(key: ConsecutiveKey<'_>, text: &str) -> CharacterCell {
    CharacterCell {
        text: text.to_string(),
        color: key.color.to_string(),
        background_color: "background".into(),
        bold: key.bold,
        italics: key.italics,
        underscore: key.underscore,
        strikethrough: key.strikethrough,
    }
}

pub fn resize_template(template: &[u8], geometry: (u16, u16)) -> Result<Element> {
    let (columns, rows) = geometry;
    let viewbox_width = columns as u32 * CELL_WIDTH as u32;
    let viewbox_height = rows as u32 * CELL_HEIGHT as u32;

    let parsed = Element::parse(template);
    let mut root = match parsed {
        Ok(element) => element,
        Err(_) => {
            let mut root = Element::new("svg");
            root.attributes.insert("viewBox".into(), format!("0 0 {} {}", viewbox_width, viewbox_height));
            root
        }
    };

    root.attributes
        .insert("viewBox".into(), format!("0 0 {} {}", viewbox_width, viewbox_height));
    root.attributes.insert("width".into(), viewbox_width.to_string());
    root.attributes.insert("height".into(), viewbox_height.to_string());

    // ensure screen svg exists
    let mut has_screen = false;
    for child in root.children.iter_mut() {
        if let XMLNode::Element(elem) = child {
            if elem.name == "svg" && elem.attributes.get("id") == Some(&"screen".to_string()) {
                has_screen = true;
                elem.attributes
                    .insert("viewBox".into(), format!("0 0 {} {}", viewbox_width, viewbox_height));
                elem.attributes.insert("width".into(), viewbox_width.to_string());
                elem.attributes.insert("height".into(), viewbox_height.to_string());
            }
        }
    }
    if !has_screen {
        let mut screen = Element::new("svg");
        screen.attributes.insert("id".into(), "screen".into());
        screen.attributes.insert("viewBox".into(), format!("0 0 {} {}", viewbox_width, viewbox_height));
        screen.attributes.insert("width".into(), viewbox_width.to_string());
        screen.attributes.insert("height".into(), viewbox_height.to_string());
        root.children.push(XMLNode::Element(screen));
    }

    ensure_defaults(&mut root, columns, rows, viewbox_width, viewbox_height);
    Ok(root)
}

fn ensure_defaults(root: &mut Element, columns: u16, rows: u16, viewbox_width: u32, viewbox_height: u32) {
    let mut has_defs = false;
    for child in root.children.iter_mut() {
        if let XMLNode::Element(elem) = child {
            if elem.name == "defs" {
                has_defs = true;
                ensure_template_settings(elem, columns, rows);
                ensure_style(elem);
                ensure_script(elem);
            }
        }
    }
    if !has_defs {
        let mut defs = Element::new("defs");
        ensure_template_settings(&mut defs, columns, rows);
        ensure_style(&mut defs);
        ensure_script(&mut defs);
        root.children.push(XMLNode::Element(defs));
    }
    root.attributes
        .insert("viewBox".into(), format!("0 0 {} {}", viewbox_width, viewbox_height));
    root.attributes.insert("width".into(), viewbox_width.to_string());
    root.attributes.insert("height".into(), viewbox_height.to_string());
}

fn ensure_style(defs: &mut Element) {
    if defs
        .children
        .iter()
        .any(|c| matches!(c, XMLNode::Element(e) if e.name == "style" && e.attributes.get("id") == Some(&"generated-style".to_string())))
    {
        return;
    }
    let mut style = Element::new("style");
    style.attributes.insert("id".into(), "generated-style".into());
    defs.children.push(XMLNode::Element(style));
}

fn ensure_script(defs: &mut Element) {
    if defs
        .children
        .iter()
        .any(|c| matches!(c, XMLNode::Element(e) if e.name == "script" && e.attributes.get("id") == Some(&"generated-js".to_string())))
    {
        return;
    }
    let mut script = Element::new("script");
    script.attributes.insert("id".into(), "generated-js".into());
    defs.children.push(XMLNode::Element(script));
}

fn ensure_template_settings(defs: &mut Element, columns: u16, rows: u16) {
    if defs
        .children
        .iter()
        .any(|c| matches!(c, XMLNode::Element(e) if e.name == "template_settings"))
    {
        return;
    }
    let mut settings = Element::new("template_settings");
    let mut geom = Element::new("screen_geometry");
    geom.attributes.insert("columns".into(), columns.to_string());
    geom.attributes.insert("rows".into(), rows.to_string());
    settings.children.push(XMLNode::Element(geom));

    let mut animation = Element::new("animation");
    animation.attributes.insert("type".into(), "css".into());
    settings.children.push(XMLNode::Element(animation));

    defs.children.push(XMLNode::Element(settings));
}

fn serialize_element(element: &Element) -> String {
    let mut buf = Vec::new();
    element.write(&mut buf).unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

fn find_style_mut(root: &mut Element) -> Option<&mut Element> {
    for node in root.children.iter_mut() {
        if let XMLNode::Element(elem) = node {
            if elem.name == "style" && elem.attributes.get("id") == Some(&"generated-style".to_string()) {
                return Some(elem);
            }
            if elem.name == "defs" {
                for child in elem.children.iter_mut() {
                    if let XMLNode::Element(e) = child {
                        if e.name == "style" && e.attributes.get("id") == Some(&"generated-style".to_string()) {
                            return Some(e);
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn validate_template(name: &str, templates: &std::collections::HashMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    if let Some(bytes) = templates.get(name) {
        return Ok(bytes.clone());
    }
    let path = Path::new(name);
    let data = std::fs::read(path)?;
    Ok(data)
}

pub fn embed_css(root: &mut Element, timings: Option<&BTreeMap<u64, i32>>, animation_duration: Option<u64>) -> Result<()> {
    let Some(style) = find_style_mut(root) else {
        return Err(anyhow!("Missing <style id=\"generated-style\"> element"));
    };

    let base_css = "#screen {\n                font-family: 'DejaVu Sans Mono', monospace;\n                font-style: normal;\n                font-size: 14px;\n                line-height: 17px;\n            }\n\n        text {\n            dominant-baseline: text-before-edge;\n            white-space: pre;\n        }\n    ";

    let final_css = if let (Some(timings), Some(duration)) = (timings, animation_duration) {
        if duration == 0 {
            return Err(anyhow!("Animation duration must be greater than 0"));
        }
        let mut transforms = Vec::new();
        let mut last_offset = 0_i32;
        for (time, offset) in timings.iter() {
            last_offset = *offset;
            let percent = 100.0 * (*time as f64) / duration as f64;
            transforms.push(format!("{percent:.3}%{{transform:translateY({offset}px)}}"));
        }
        transforms.push(format!("100%{{transform:translateY({last_offset}px)}}"));
        format!(
            "{base_css}\n:root {{ --animation-duration: {duration}ms; }}\n@keyframes roll {{{transforms}}}\n#screen_view {{ animation-duration: {duration}ms; animation-iteration-count:infinite; animation-name:roll; animation-timing-function: steps(1,end); animation-fill-mode: forwards; }}",
            base_css = base_css,
            duration = duration,
            transforms = transforms.join(" ")
        )
    } else {
        base_css.to_string()
    };

    style.children.clear();
    style.children.push(XMLNode::Text(final_css));
    Ok(())
}

pub fn embed_waapi(root: &mut Element, timings: Option<&BTreeMap<u64, i32>>, animation_duration: Option<u64>) -> Result<()> {
    embed_css(root, timings, animation_duration)
}

pub fn validate_svg<T: AsRef<[u8]>>(svg_data: T) -> Result<()> {
    validate_svg_bytes(svg_data.as_ref())
}

fn validate_svg_bytes(bytes: &[u8]) -> Result<()> {
    Element::parse(bytes).map(|_| ())?;
    Ok(())
}
