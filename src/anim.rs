use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::str;

use anyhow::{Result, anyhow};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use roxmltree::Document;
use unicode_width::UnicodeWidthStr;
use xmltree::{Element, EmitterConfig, XMLNode};

use crate::term::TimedFrame;

pub const CELL_WIDTH: u16 = 8;
pub const CELL_HEIGHT: u16 = 17;
const FRAME_CELL_SPACING: i32 = 1;
const DEFAULT_TERMTOSVG_NS: &str = {
    match option_env!("CARGO_PKG_REPOSITORY") {
        Some(v) if !v.is_empty() => v,
        _ => "",
    }
};

type DefinitionMap = IndexMap<DefinitionKey, Element>;

fn termtosvg_namespace() -> String {
    std::env::var("TERMTOSVG_NAMESPACE").unwrap_or_else(|_| DEFAULT_TERMTOSVG_NS.to_string())
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DefinitionKey(Vec<TextRunKey>);

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextRunKey {
    column: usize,
    text: String,
    color: String,
    use_fill_attribute: bool,
    bold: bool,
    italics: bool,
    underscore: bool,
    strikethrough: bool,
}

impl TextRunKey {
    fn from_character(column: usize, text: &str, cell: &CharacterCell) -> Self {
        TextRunKey {
            column,
            text: text.to_string(),
            color: cell.color.clone(),
            use_fill_attribute: cell.color.starts_with('#'),
            bold: cell.bold,
            italics: cell.italics,
            underscore: cell.underscore,
            strikethrough: cell.strikethrough,
        }
    }
}

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

#[derive(Clone, Copy, PartialEq, Eq)]
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
        text_elem
            .attributes
            .insert("font-weight".into(), "bold".into());
    }
    if attr.italics {
        text_elem
            .attributes
            .insert("font-style".into(), "italic".into());
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
        text_elem
            .attributes
            .insert("fill".into(), attr.color.clone());
    } else {
        text_elem
            .attributes
            .insert("class".into(), attr.color.clone());
    }

    text_elem.children.push(XMLNode::Text(text.to_string()));
    text_elem
}

fn make_rect_tag(
    column: usize,
    length: usize,
    height: i32,
    cell_width: u16,
    cell_height: u16,
    background_color: &str,
) -> Element {
    let mut rect = Element::new("rect");
    rect.attributes
        .insert("x".into(), (column as u32 * cell_width as u32).to_string());
    rect.attributes.insert("y".into(), height.to_string());
    rect.attributes.insert(
        "width".into(),
        (length as u32 * cell_width as u32).to_string(),
    );
    rect.attributes
        .insert("height".into(), cell_height.to_string());

    if background_color.starts_with('#') {
        rect.attributes
            .insert("fill".into(), background_color.into());
    } else {
        rect.attributes
            .insert("class".into(), background_color.into());
    }

    rect
}

pub fn render_animation<I: IntoIterator<Item = TimedFrame>, P: AsRef<Path>>(
    frames: I,
    geometry: (u16, u16),
    filename: P,
    template: &[u8],
) -> Result<()> {
    let mut root = render_preparation(geometry, template)?;
    ensure_css_animation(&root)?;
    let screen_height = geometry.1;
    let mut screen_view = Element::new("g");
    screen_view
        .attributes
        .insert("id".into(), "screen_view".into());

    let mut definitions: DefinitionMap = DefinitionMap::new();
    let mut timings: BTreeMap<u64, i32> = BTreeMap::new();
    let mut animation_duration: Option<u64> = None;
    let frame_stride = frame_vertical_stride(screen_height);

    for (frame_idx, frame) in frames.into_iter().enumerate() {
        let offset = frame_idx as i32 * frame_stride;
        let (group, defs) =
            render_timed_frame(offset, &frame.buffer, CELL_HEIGHT, CELL_WIDTH, &definitions);
        definitions.extend(defs.into_iter());
        animation_duration = Some(frame.time + frame.duration);
        timings.insert(frame.time, -offset);
        screen_view.children.push(XMLNode::Element(group));
    }

    {
        let screen = find_screen_mut(&mut root)
            .ok_or_else(|| anyhow!("Missing <svg id=\"screen\"> element in template"))?;
        let mut defs_elem = Element::new("defs");
        for def in definitions.values() {
            defs_elem.children.push(XMLNode::Element(def.clone()));
        }
        screen.children.push(XMLNode::Element(defs_elem));
        screen.children.push(XMLNode::Element(screen_view));
    }

    let timings_option = if animation_duration.is_some() && !timings.is_empty() {
        Some(&timings)
    } else {
        None
    };
    embed_css(&mut root, timings_option, animation_duration)?;

    let mut file = File::create(filename)?;
    let bytes = emit_svg_bytes(&root)?;
    file.write_all(&bytes)?;
    Ok(())
}

pub fn render_still_frames<I: IntoIterator<Item = TimedFrame>, P: AsRef<Path>>(
    frames: I,
    geometry: (u16, u16),
    directory: P,
    template: &[u8],
) -> Result<()> {
    std::fs::create_dir_all(directory.as_ref())?;
    let root = render_preparation(geometry, template)?;
    ensure_css_animation(&root)?;
    let empty_defs = DefinitionMap::new();
    for (idx, frame) in frames.into_iter().enumerate() {
        let mut frame_root = root.clone();
        let (group, defs) =
            render_timed_frame(0, &frame.buffer, CELL_HEIGHT, CELL_WIDTH, &empty_defs);

        {
            let screen = find_screen_mut(&mut frame_root)
                .ok_or_else(|| anyhow!("Missing <svg id=\"screen\"> element in template"))?;
            let mut defs_elem = Element::new("defs");
            for def in defs.values() {
                defs_elem.children.push(XMLNode::Element(def.clone()));
            }
            screen.children.push(XMLNode::Element(defs_elem));
            screen.children.push(XMLNode::Element(group));
        }

        embed_css(&mut frame_root, None, None)?;

        let filename = directory.as_ref().join(format!("termtosvg_{:05}.svg", idx));
        let mut file = File::create(filename)?;
        let bytes = emit_svg_bytes(&frame_root)?;
        file.write_all(&bytes)?;
    }
    Ok(())
}

pub fn render_preparation(geometry: (u16, u16), template: &[u8]) -> Result<Element> {
    let mut root = resize_template(template, geometry)?;
    // clear previous screen content and ensure bg rect exists
    if let Some(screen) = find_screen_mut(&mut root) {
        screen.children.clear();
        screen.children.push(XMLNode::Element(BG_RECT.clone()));
    } else {
        return Err(anyhow!("Missing <svg id=\"screen\"> element in template"));
    }
    Ok(root)
}

pub fn render_timed_frame(
    offset: i32,
    buffer: &BTreeMap<usize, BTreeMap<usize, CharacterCell>>,
    cell_height: u16,
    cell_width: u16,
    definitions: &DefinitionMap,
) -> (Element, DefinitionMap) {
    let mut frame_group = Element::new("g");
    let mut new_definitions: DefinitionMap = DefinitionMap::new();

    for (row, line) in buffer.iter() {
        if line.is_empty() {
            continue;
        }
        let (mut tags, defs) = render_line(
            offset,
            *row,
            line,
            cell_height,
            cell_width,
            definitions,
            &new_definitions,
        );
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
    definitions: &DefinitionMap,
    local_defs: &DefinitionMap,
) -> (Vec<XMLNode>, DefinitionMap) {
    let mut nodes = Vec::new();
    let cell_height_i32 = i32::from(cell_height);
    let line_position = offset + row as i32 * cell_height_i32;

    // backgrounds
    for rect in render_line_bg_colors(row_data, line_position, cell_height, cell_width) {
        nodes.push(XMLNode::Element(rect));
    }

    // text group
    let (mut text_group, definition_key) = render_characters_with_key(row_data, cell_width);
    let mut new_defs = DefinitionMap::new();
    let mut group_id = None;
    if let Some(existing) = definitions
        .get(&definition_key)
        .or_else(|| local_defs.get(&definition_key))
    {
        group_id = existing.attributes.get("id").cloned();
    }
    if group_id.is_none() {
        let id = format!("g{}", definitions.len() + local_defs.len() + 1);
        text_group.attributes.insert("id".into(), id.clone());
        new_defs.insert(definition_key, text_group);
        group_id = Some(id);
    }
    if let Some(id) = group_id {
        let mut use_tag = Element::new("use");
        use_tag
            .attributes
            .insert("xlink:href".into(), format!("#{id}"));
        use_tag
            .attributes
            .insert("y".into(), line_position.to_string());
        nodes.push(XMLNode::Element(use_tag));
    }

    (nodes, new_defs)
}

pub fn render_line_bg_colors(
    row: &BTreeMap<usize, CharacterCell>,
    height: i32,
    cell_height: u16,
    cell_width: u16,
) -> Vec<Element> {
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
            rects.push(make_rect_tag(
                start,
                width,
                height,
                cell_width,
                cell_height,
                color,
            ));
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

fn render_characters_with_key(
    row: &BTreeMap<usize, CharacterCell>,
    cell_width: u16,
) -> (Element, DefinitionKey) {
    let mut text_group = Element::new("g");
    let mut run_keys = Vec::new();
    if row.is_empty() {
        return (text_group, DefinitionKey(run_keys));
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
                append_text_run(
                    &mut text_group,
                    &mut run_keys,
                    current_col,
                    prev_key,
                    &current_text,
                    cell_width,
                );
            }
            current_key = Some(key);
            current_col = col;
            current_text.clear();
            current_text.push_str(&cell.text);
        }
        last_col = Some(col);
    }

    if let Some(prev_key) = current_key {
        append_text_run(
            &mut text_group,
            &mut run_keys,
            current_col,
            prev_key,
            &current_text,
            cell_width,
        );
    }

    (text_group, DefinitionKey(run_keys))
}

fn append_text_run(
    text_group: &mut Element,
    run_keys: &mut Vec<TextRunKey>,
    column: usize,
    run_key: ConsecutiveKey<'_>,
    text: &str,
    cell_width: u16,
) {
    if text.is_empty() {
        return;
    }
    let grouped_cell = cell_from_key(run_key, text);
    run_keys.push(TextRunKey::from_character(column, text, &grouped_cell));
    let elem = make_text_tag(column, &grouped_cell, text, cell_width);
    text_group.children.push(XMLNode::Element(elem));
}

pub fn render_characters(row: &BTreeMap<usize, CharacterCell>, cell_width: u16) -> Element {
    render_characters_with_key(row, cell_width).0
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

fn frame_vertical_stride(rows: u16) -> i32 {
    let base = i32::from(rows) + FRAME_CELL_SPACING;
    let even = base + (base % 2);
    even * i32::from(CELL_HEIGHT)
}

pub fn resize_template(template: &[u8], geometry: (u16, u16)) -> Result<Element> {
    let mut root = Element::parse(template).map_err(|_| anyhow!("Invalid template"))?;
    let (template_columns, template_rows) = ensure_template_defaults(&mut root, geometry)?;
    let delta_columns = i32::from(geometry.0) - i32::from(template_columns);
    let delta_rows = i32::from(geometry.1) - i32::from(template_rows);

    apply_scale(&mut root, delta_columns, delta_rows)?;
    {
        let screen = find_screen_mut(&mut root)
            .ok_or_else(|| anyhow!("Missing <svg id=\"screen\"> element in template"))?;
        apply_scale(screen, delta_columns, delta_rows)?;
    }

    Ok(root)
}

fn ensure_template_defaults(root: &mut Element, geometry: (u16, u16)) -> Result<(u16, u16)> {
    let defs_index = find_or_create_defs_index(root);
    let (template_columns, template_rows) = match root.children.get_mut(defs_index) {
        Some(XMLNode::Element(defs)) => {
            let values = ensure_template_settings(defs, geometry.0, geometry.1)?;
            ensure_style(defs);
            values
        }
        _ => return Err(anyhow!("Unable to locate <defs> element in template")),
    };
    Ok((template_columns, template_rows))
}

fn find_or_create_defs_index(root: &mut Element) -> usize {
    if let Some((idx, _)) =
        root.children.iter().enumerate().find(
            |(_, child)| matches!(child, XMLNode::Element(elem) if node_name_eq(elem, "defs")),
        )
    {
        return idx;
    }
    root.children.push(XMLNode::Element(Element::new("defs")));
    root.children.len() - 1
}

fn ensure_template_settings(defs: &mut Element, columns: u16, rows: u16) -> Result<(u16, u16)> {
    let namespace = termtosvg_namespace();
    let settings = ensure_child(defs, "template_settings", || {
        Element::new("termtosvg:template_settings")
    });
    if !namespace.is_empty() {
        apply_namespace(settings, namespace.as_str());
    }

    let _animation = ensure_child(settings, "animation", || {
        let mut element = Element::new("termtosvg:animation");
        element.attributes.insert("type".into(), "css".into());
        element
    });

    let screen_geometry = ensure_child(settings, "screen_geometry", || {
        let mut element = Element::new("termtosvg:screen_geometry");
        element
            .attributes
            .insert("columns".into(), columns.to_string());
        element.attributes.insert("rows".into(), rows.to_string());
        element
    });

    let template_columns = screen_geometry
        .attributes
        .get("columns")
        .ok_or_else(|| anyhow!("Missing \"columns\" attribute in screen_geometry"))?
        .parse::<u16>()
        .map_err(|_| anyhow!("Invalid \"columns\" attribute in screen_geometry"))?;
    let template_rows = screen_geometry
        .attributes
        .get("rows")
        .ok_or_else(|| anyhow!("Missing \"rows\" attribute in screen_geometry"))?
        .parse::<u16>()
        .map_err(|_| anyhow!("Invalid \"rows\" attribute in screen_geometry"))?;

    screen_geometry
        .attributes
        .insert("columns".into(), columns.to_string());
    screen_geometry
        .attributes
        .insert("rows".into(), rows.to_string());

    Ok((template_columns, template_rows))
}

fn ensure_child<'a>(
    parent: &'a mut Element,
    name: &str,
    builder: impl FnOnce() -> Element,
) -> &'a mut Element {
    if let Some(idx) = parent
        .children
        .iter()
        .enumerate()
        .find_map(|(idx, child)| match child {
            XMLNode::Element(elem) if node_name_eq(elem, name) => Some(idx),
            _ => None,
        })
    {
        match parent.children.get_mut(idx) {
            Some(XMLNode::Element(elem)) => return elem,
            _ => unreachable!("Matched child must be an element"),
        }
    }
    parent.children.push(XMLNode::Element(builder()));
    match parent.children.last_mut() {
        Some(XMLNode::Element(elem)) => elem,
        _ => unreachable!("Newly inserted child must be an element"),
    }
}

fn apply_namespace(element: &mut Element, namespace: &str) {
    if let Some(ns_map) = element.namespaces.take() {
        let mut ns_map = ns_map;
        ns_map.force_put("termtosvg", namespace);
        element.namespaces = Some(ns_map);
    }
    if let Some(attr) = element.attributes.get_mut("xmlns:termtosvg") {
        *attr = namespace.to_string();
    }
    for child in element.children.iter_mut() {
        if let XMLNode::Element(elem) = child {
            apply_namespace(elem, namespace);
        }
    }
}

fn find_child_element<'a>(parent: &'a Element, name: &str) -> Option<&'a Element> {
    parent.children.iter().find_map(|child| match child {
        XMLNode::Element(elem) if node_name_eq(elem, name) => Some(elem),
        _ => None,
    })
}

fn ensure_css_animation(root: &Element) -> Result<()> {
    let defs = find_child_element(root, "defs")
        .ok_or_else(|| anyhow!("Unable to locate <defs> element in template"))?;
    let settings = find_child_element(defs, "template_settings")
        .ok_or_else(|| anyhow!("Missing \"template_settings\" element in definitions"))?;
    let animation = find_child_element(settings, "animation")
        .ok_or_else(|| anyhow!("Missing \"animation\" element in \"template_settings\""))?;
    let animation_type = animation
        .attributes
        .get("type")
        .ok_or_else(|| anyhow!("Missing \"type\" attribute for animation element"))?
        .to_lowercase();

    if animation_type == "css" {
        return Ok(());
    }

    Err(anyhow!(
        "Template requests unsupported animation type '{animation_type}'. Only CSS-based templates are supported (GitHub-safe).",
    ))
}

fn ensure_style(defs: &mut Element) {
    if defs.children.iter().any(|child| matches!(child, XMLNode::Element(elem) if elem.name == "style" && elem.attributes.get("id") == Some(&"generated-style".to_string()))) {
        return;
    }
    let mut style = Element::new("style");
    style
        .attributes
        .insert("id".into(), "generated-style".into());
    style.attributes.insert("type".into(), "text/css".into());
    defs.children.push(XMLNode::Element(style));
}

fn node_name_eq(element: &Element, expected: &str) -> bool {
    if element.name == expected {
        return true;
    }
    element
        .name
        .rsplit_once(':')
        .map(|(_, local)| local == expected)
        .unwrap_or(false)
}

fn find_screen_mut(element: &mut Element) -> Option<&mut Element> {
    if element.name == "svg"
        && element
            .attributes
            .get("id")
            .map(|id| id == "screen")
            .unwrap_or(false)
    {
        return Some(element);
    }
    for child in element.children.iter_mut() {
        if let XMLNode::Element(elem) = child
            && let Some(found) = find_screen_mut(elem)
        {
            return Some(found);
        }
    }
    None
}

fn apply_scale(element: &mut Element, delta_columns: i32, delta_rows: i32) -> Result<()> {
    if delta_columns == 0 && delta_rows == 0 {
        return Ok(());
    }
    let viewbox_value = element
        .attributes
        .get("viewBox")
        .ok_or_else(|| anyhow!("Missing \"viewBox\" attribute"))?
        .replace(',', " ");
    let parts: Vec<&str> = viewbox_value.split_whitespace().collect();
    if parts.len() != 4 {
        return Err(anyhow!("Invalid viewBox attribute"));
    }
    let min_x: i32 = parts[0]
        .parse()
        .map_err(|_| anyhow!("Invalid viewBox attribute"))?;
    let min_y: i32 = parts[1]
        .parse()
        .map_err(|_| anyhow!("Invalid viewBox attribute"))?;
    let mut width: i32 = parts[2]
        .parse()
        .map_err(|_| anyhow!("Invalid viewBox attribute"))?;
    let mut height: i32 = parts[3]
        .parse()
        .map_err(|_| anyhow!("Invalid viewBox attribute"))?;

    width += delta_columns * i32::from(CELL_WIDTH);
    height += delta_rows * i32::from(CELL_HEIGHT);

    element.attributes.insert(
        "viewBox".into(),
        format!("{min_x} {min_y} {width} {height}"),
    );

    adjust_numeric_attr(element, "width", delta_columns * i32::from(CELL_WIDTH))?;
    adjust_numeric_attr(element, "height", delta_rows * i32::from(CELL_HEIGHT))?;
    Ok(())
}

fn adjust_numeric_attr(element: &mut Element, name: &str, delta: i32) -> Result<()> {
    if delta == 0 {
        return Ok(());
    }
    if let Some(value) = element.attributes.get_mut(name) {
        let current: i32 = value
            .parse()
            .map_err(|_| anyhow!("Attribute {name} must be numeric"))?;
        *value = (current + delta).to_string();
    }
    Ok(())
}

fn emit_svg_bytes(element: &Element) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    element.write_with_config(
        &mut buf,
        EmitterConfig::new().write_document_declaration(false),
    )?;
    validate_svg_bytes(&buf)?;
    Ok(buf)
}

fn find_style_mut(root: &mut Element) -> Option<&mut Element> {
    for node in root.children.iter_mut() {
        if let XMLNode::Element(elem) = node {
            if elem.name == "style"
                && matches!(elem.attributes.get("id"), Some(value) if value == "generated-style")
            {
                return Some(elem);
            }
            if elem.name == "defs" {
                for child in elem.children.iter_mut() {
                    if let XMLNode::Element(e) = child
                        && e.name == "style"
                        && matches!(e.attributes.get("id"), Some(value) if value == "generated-style")
                    {
                        return Some(e);
                    }
                }
            }
        }
    }
    None
}

pub fn validate_template(
    name: &str,
    templates: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>> {
    if let Some(bytes) = templates.get(name) {
        return Ok(bytes.clone());
    }
    let path = Path::new(name);
    let data = std::fs::read(path)?;
    Ok(data)
}

const CSS_BODY: &str = "#screen {\n                font-family: 'DejaVu Sans Mono', monospace;\n                font-style: normal;\n                font-size: 14px;\n            }\n\n        text {\n            dominant-baseline: text-before-edge;\n            white-space: pre;\n        }\n    \n";

pub fn embed_css(
    root: &mut Element,
    timings: Option<&BTreeMap<u64, i32>>,
    animation_duration: Option<u64>,
) -> Result<()> {
    let Some(style) = find_style_mut(root) else {
        return Err(anyhow!("Missing <style id=\"generated-style\"> element"));
    };

    let mut final_css = CSS_BODY.to_string();
    if let (Some(timings), Some(duration)) = (timings, animation_duration) {
        if duration == 0 {
            return Err(anyhow!("Animation duration must be greater than 0"));
        }
        let mut transforms = Vec::new();
        let mut last_offset: Option<i32> = None;
        for (time, offset) in timings.iter() {
            let percent = 100.0 * (*time as f64) / duration as f64;
            transforms.push(format!("{percent:.3}%{{transform:translateY({offset}px)}}"));
            last_offset = Some(*offset);
        }
        if let Some(offset) = last_offset {
            transforms.push(format!("100.000%{{transform:translateY({offset}px)}}"));
        }

        let transform_block = transforms.join("\n");
        let css_animation = format!(
            "            :root {{\n                --animation-duration: {duration}ms;\n            }}\n\n            @keyframes roll {{\n                {transform_block}\n            }}\n\n            #screen_view {{\n                animation-duration: {duration}ms;\n                animation-iteration-count:infinite;\n                animation-name:roll;\n                animation-timing-function: steps(1,end);\n                animation-fill-mode: forwards;\n            }}\n        ",
            duration = duration,
            transform_block = transform_block
        );
        final_css.push_str(&css_animation);
    }

    style.children.clear();
    style.children.push(XMLNode::CData(final_css));
    Ok(())
}

pub fn validate_svg<T: AsRef<[u8]>>(svg_data: T) -> Result<()> {
    validate_svg_bytes(svg_data.as_ref())
}

fn validate_svg_bytes(bytes: &[u8]) -> Result<()> {
    let svg_text =
        str::from_utf8(bytes).map_err(|err| anyhow!("SVG output is not valid UTF-8: {err}"))?;
    Document::parse(svg_text).map_err(|err| anyhow!("Invalid SVG emitted: {err}"))?;
    Ok(())
}
