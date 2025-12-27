use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::process::{Command, Stdio};

use assert_cmd::prelude::*;
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
    let null_input = std::fs::File::open("/dev/null").expect("open /dev/null for reading");
    let null_output = std::fs::File::options()
        .write(true)
        .open("/dev/null")
        .expect("open /dev/null for writing");

    let cast_dir = tempdir().unwrap();
    let cast_path = cast_dir.path().join("session.cast");

    let command = "/bin/sh -c 'printf record_flow'".to_string();
    let mut record_args = vec![
        "termtosvg".to_string(),
        "record".to_string(),
        "-c".to_string(),
        command,
    ];
    record_args.push(cast_path.display().to_string());
    cli::run(record_args, null_input.as_raw_fd(), null_output.as_raw_fd())
        .expect("record succeeds");

    let render_dir = tempdir().unwrap();
    let render_path = render_dir.path().join("output.svg");

    let render_args = vec![
        "termtosvg".to_string(),
        "render".to_string(),
        cast_path.display().to_string(),
        render_path.display().to_string(),
    ];
    cli::run(render_args, null_input.as_raw_fd(), null_output.as_raw_fd())
        .expect("render succeeds");

    assert!(render_path.exists());
    assert_valid_svg(&render_path);
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
    assert_valid_svg(&svg_path);
}

#[test]
fn render_honors_namespace_override() {
    let cast = sample_cast_file();
    let output_dir = tempdir().unwrap();
    let svg_path = output_dir.path().join("override.svg");
    let template = "data/templates/powershell.svg";
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

    assert_valid_svg(&svg_path);
    let svg = std::fs::read_to_string(&svg_path).unwrap();
    assert!(svg.contains(&format!("xmlns:termtosvg=\"{namespace}\"")));
}

#[test]
fn default_command_reads_piped_stdin() {
    let output_dir = tempdir().unwrap();
    let svg_path = output_dir.path().join("stdin.svg");
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("termtosvg"))
        .args(["-g", "20x5", "-o", svg_path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn termtosvg");

    {
        let stdin = child.stdin.as_mut().expect("child stdin available");
        writeln!(stdin, "\u{1b}[31mhello from stdin\u{1b}[0m").unwrap();
    }

    assert!(child.wait().unwrap().success());
    assert_valid_svg(&svg_path);
    let svg = std::fs::read_to_string(&svg_path).unwrap();
    assert!(svg.contains("hello from stdin"));
}

#[test]
fn render_subcommand_accepts_dash_stdin() {
    let output_dir = tempdir().unwrap();
    let svg_path = output_dir.path().join("dash.svg");
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("termtosvg"))
        .args(["render", "-", svg_path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn termtosvg render -");

    {
        let stdin = child.stdin.as_mut().expect("child stdin available");
        writeln!(
            stdin,
            "{}",
            serde_json::json!({"version":2,"width":80,"height":24})
        )
        .unwrap();
        writeln!(stdin, "{}", serde_json::json!([0.0, "o", "dash stdin"])).unwrap();
    }

    assert!(child.wait().unwrap().success());
    assert_valid_svg(&svg_path);
    let svg = std::fs::read_to_string(&svg_path).unwrap();
    assert!(svg.contains("dash stdin"));
}

fn assert_valid_svg(path: &std::path::Path) {
    let xml = std::fs::read_to_string(path).expect("read SVG");
    roxmltree::Document::parse(&xml).unwrap_or_else(|err| {
        panic!("Invalid SVG emitted at {}: {err}", path.display());
    });
}
