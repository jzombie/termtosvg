use assert_cmd::prelude::*;
use std::error::Error;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

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
    assert_eq!(
        rust_bytes, python_bytes,
        "SVG output differs from Python reference"
    );
    Ok(())
}
