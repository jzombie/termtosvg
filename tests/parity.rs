use assert_cmd::prelude::*;
use std::collections::BTreeMap;
use std::error::Error;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use xmltree::{Element, XMLNode};

#[test]
#[ignore = "requires Python reference renderer"]
fn rust_svg_matches_python_reference() -> Result<(), Box<dyn Error>> {
    let reference_bin = std::env::var("TERMTOSVG_REFERENCE_BIN")
        .unwrap_or_else(|_| ".venv/bin/termtosvg".to_string());
    let reference_path = Path::new(&reference_bin);
    if !reference_path.exists() {
        panic!(
            "Reference renderer not found at {} (set TERMTOSVG_REFERENCE_BIN)",
            reference_path.display()
        );
    }

    let template_path = Path::new("old.python/termtosvg/data/templates/powershell.svg");
    assert!(
        template_path.exists(),
        "Missing template at {}",
        template_path.display()
    );

    let temp = tempdir()?;
    let cast_path = temp.path().join("parity.cast");
    std::fs::write(&cast_path, include_str!("../samples.cast"))?;
    let python_svg = temp.path().join("python.svg");
    let rust_svg = temp.path().join("rust.svg");

    let cast_str = cast_path.to_string_lossy().to_string();
    let python_svg_str = python_svg.to_string_lossy().to_string();
    let rust_svg_str = rust_svg.to_string_lossy().to_string();
    let template_str = template_path.to_string_lossy().to_string();

    let status = Command::new(reference_path)
        .args(["render", &cast_str, &python_svg_str, "-t", &template_str])
        .status()?;
    assert!(status.success(), "Python reference renderer failed");

    Command::new(assert_cmd::cargo::cargo_bin!("termtosvg"))
        .args([
            "render",
            &cast_str,
            "--output",
            &rust_svg_str,
            "--template",
            &template_str,
        ])
        .assert()
        .success();

    let python_bytes = std::fs::read(&python_svg)?;
    let rust_bytes = std::fs::read(&rust_svg)?;
    let python_dom = canonicalize_svg(&python_bytes)?;
    let rust_dom = canonicalize_svg(&rust_bytes)?;
    assert_eq!(
        rust_dom, python_dom,
        "SVG DOM differs from Python reference"
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
            if text.trim().is_empty() {
                None
            } else {
                Some(CanonNode::Text(text.clone()))
            }
        }
        XMLNode::CData(data) => Some(CanonNode::CData(data.clone())),
        _ => None,
    }
}
