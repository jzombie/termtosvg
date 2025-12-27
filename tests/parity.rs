use assert_cmd::prelude::*;
use std::collections::BTreeMap;
use std::error::Error;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use termtosvg::config::DEFAULT_TEMPLATES_NAMES;
use xmltree::{Element, XMLNode};

const PYTHON_NAMESPACE: &str = "https://github.com/nbedos/termtosvg";
const TEMPLATE_ROOT: &str = "data/templates";

#[test]
#[ignore = "requires Python reference renderer"]
fn rust_svg_matches_python_reference_default_templates() -> Result<(), Box<dyn Error>> {
    compare_against_python(DEFAULT_TEMPLATES_NAMES)
}

fn compare_against_python(templates: &[&str]) -> Result<(), Box<dyn Error>> {
    let reference_bin = std::env::var("TERMTOSVG_REFERENCE_BIN")
        .unwrap_or_else(|_| ".venv/bin/termtosvg".to_string());
    let reference_path = Path::new(&reference_bin);
    if !reference_path.exists() {
        panic!(
            "Reference renderer not found at {} (set TERMTOSVG_REFERENCE_BIN)",
            reference_path.display()
        );
    }

    let template_root = Path::new(TEMPLATE_ROOT);
    let temp_dir = tempdir()?;
    let cast_path = temp_dir.path().join("parity.cast");
    std::fs::write(&cast_path, include_str!("../samples.cast"))?;
    let cast_str = cast_path.to_string_lossy().to_string();

    for template_name in templates {
        let template_path = template_root.join(template_name);
        assert!(
            template_path.exists(),
            "Missing template at {}",
            template_path.display()
        );

        let slug = template_name.trim_end_matches(".svg");
        let python_svg = temp_dir.path().join(format!("python-{slug}.svg"));
        let rust_svg = temp_dir.path().join(format!("rust-{slug}.svg"));
        render_with_python(
            reference_path,
            &cast_str,
            &python_svg,
            &template_path,
            template_name,
        )?;
        render_with_rust(&cast_str, &rust_svg, &template_path)?;
        compare_outputs(&python_svg, &rust_svg, template_name)?;
    }
    Ok(())
}

fn render_with_python(
    reference_path: &Path,
    cast_file: &str,
    output_svg: &Path,
    template_path: &Path,
    template_name: &str,
) -> Result<(), Box<dyn Error>> {
    let output_str = output_svg.to_string_lossy().to_string();
    let template_str = template_path.to_string_lossy().to_string();
    let status = Command::new(reference_path)
        .args(["render", cast_file, &output_str, "-t", &template_str])
        .status()?;
    if !status.success() {
        return Err(format!("Python renderer failed for template {template_name}").into());
    }
    Ok(())
}

fn render_with_rust(
    cast_file: &str,
    output_svg: &Path,
    template_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let output_str = output_svg.to_string_lossy().to_string();
    let template_str = template_path.to_string_lossy().to_string();
    Command::new(assert_cmd::cargo::cargo_bin!("termtosvg"))
        .args([
            "render",
            cast_file,
            "--output",
            &output_str,
            "--template",
            &template_str,
        ])
        .env("TERMTOSVG_NAMESPACE", PYTHON_NAMESPACE)
        .assert()
        .success();
    Ok(())
}

fn compare_outputs(
    python_svg: &Path,
    rust_svg: &Path,
    template_name: &str,
) -> Result<(), Box<dyn Error>> {
    assert_valid_svg(python_svg)?;
    assert_valid_svg(rust_svg)?;
    let python_bytes = std::fs::read(python_svg)?;
    let rust_bytes = std::fs::read(rust_svg)?;
    let python_dom = canonicalize_svg(&python_bytes)?;
    let rust_dom = canonicalize_svg(&rust_bytes)?;
    assert_eq!(
        rust_dom, python_dom,
        "SVG DOM differs from Python reference for template {template_name}"
    );
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct CanonElement {
    name: String,
    namespace: Option<String>,
    namespaces: BTreeMap<String, String>,
    attributes: BTreeMap<String, String>,
    children: Vec<CanonNode>,
}

#[derive(Debug, PartialEq, Eq)]
enum CanonNode {
    Element(CanonElement),
    Text(String),
    CData(String),
}

fn canonicalize_svg(bytes: &[u8]) -> Result<CanonElement, Box<dyn Error>> {
    let mut cursor = Cursor::new(bytes);
    let element = Element::parse(&mut cursor)?;
    Ok(canonicalize_element(&element))
}

fn canonicalize_element(element: &Element) -> CanonElement {
    let namespaces = element
        .namespaces
        .as_ref()
        .map(|ns| {
            ns.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .unwrap_or_default();
    CanonElement {
        name: element.name.clone(),
        namespace: element.namespace.clone(),
        namespaces,
        attributes: element
            .attributes
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        children: element
            .children
            .iter()
            .filter_map(canonicalize_node)
            .collect(),
    }
}

fn canonicalize_node(node: &XMLNode) -> Option<CanonNode> {
    match node {
        XMLNode::Element(child) => Some(CanonNode::Element(canonicalize_element(child))),
        XMLNode::Text(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(CanonNode::Text(trimmed.to_string()))
            }
        }
        // Normalize whitespace in CDATA so stylistic differences (e.g., minified vs pretty CSS)
        // don't cause false negatives when comparing semantically equivalent SVGs.
        XMLNode::CData(data) => {
            let normalized: String = data.split_whitespace().collect();
            if normalized.is_empty() {
                None
            } else {
                Some(CanonNode::CData(normalized))
            }
        }
        _ => None,
    }
}

fn assert_valid_svg(path: &Path) -> Result<(), Box<dyn Error>> {
    let xml = std::fs::read_to_string(path)?;
    roxmltree::Document::parse(&xml).map_err(|err| -> Box<dyn Error> {
        format!("Invalid SVG emitted at {}: {err}", path.display()).into()
    })?;
    Ok(())
}
