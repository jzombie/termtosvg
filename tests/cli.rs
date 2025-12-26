use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::process::Command;

use assert_cmd::prelude::*;
use nix::unistd::{close, pipe, write};
use tempfile::tempdir;

use termtosvg::cli;

fn sample_cast_file() -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("temp file");
    writeln!(
        file,
        "{}",
        serde_json::json!({"version":2,"width":80,"height":24})
    )
    .unwrap();
    writeln!(file, "{}", serde_json::json!([0.0, "o", "hello"])).unwrap();
    file
}

#[test]
fn integral_duration_validation_accepts_ms_suffix() {
    assert_eq!(cli::integral_duration_validation("100").unwrap(), 100);
    assert_eq!(cli::integral_duration_validation("100ms").unwrap(), 100);
}

#[test]
fn record_and_render_flow() {
    let (reader, writer) = pipe().expect("pipe");

    // write some data then close the writer so the reader sees EOF
    write(writer, b"echo test\n").unwrap();
    close(writer).unwrap();

    let cast_dir = tempdir().unwrap();
    let cast_path = cast_dir.path().join("session.cast");

    let mut record_args = vec!["termtosvg".to_string(), "record".to_string()];
    record_args.push(cast_path.display().to_string());
    cli::run(record_args, reader, std::io::stdout().as_raw_fd()).expect("record succeeds");

    let render_dir = tempdir().unwrap();
    let render_path = render_dir.path().join("output.svg");

    let render_args = vec![
        "termtosvg".to_string(),
        "render".to_string(),
        cast_path.display().to_string(),
        render_path.display().to_string(),
    ];
    cli::run(
        render_args,
        std::io::stdin().as_raw_fd(),
        std::io::stdout().as_raw_fd(),
    )
    .expect("render succeeds");

    assert!(render_path.exists());
}

#[test]
fn render_with_existing_cast() {
    let cast = sample_cast_file();
    let output_dir = tempdir().unwrap();
    let svg_path = output_dir.path().join("out.svg");

    let args = vec![
        "termtosvg".to_string(),
        "render".to_string(),
        cast.path().display().to_string(),
        svg_path.display().to_string(),
    ];
    cli::run(
        args,
        std::io::stdin().as_raw_fd(),
        std::io::stdout().as_raw_fd(),
    )
    .unwrap();
    assert!(svg_path.exists());
}

#[test]
fn render_honors_namespace_override() {
    let cast = sample_cast_file();
    let output_dir = tempdir().unwrap();
    let svg_path = output_dir.path().join("override.svg");
    let template = "old.python/termtosvg/data/templates/powershell.svg";
    let namespace = "https://example.com/custom-termtosvg";

    Command::new(assert_cmd::cargo::cargo_bin!("termtosvg"))
        .env("TERMTOSVG_NAMESPACE", namespace)
        .args([
            "render",
            cast.path().to_str().unwrap(),
            svg_path.to_str().unwrap(),
            "-t",
            template,
        ])
        .assert()
        .success();

    let svg = std::fs::read_to_string(&svg_path).unwrap();
    assert!(svg.contains(&format!("xmlns:termtosvg=\"{namespace}\"")));
}
