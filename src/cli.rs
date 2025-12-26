use std::fs::File;
use std::io::Write;
use std::os::unix::io::RawFd;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use rand::{distr::Alphanumeric, Rng};
use tempfile::NamedTempFile;

use crate::anim;
use crate::asciicast::{self, AsciiCastV2Record};
use crate::config;
use crate::term::{self, TimedFrame};

pub const DEFAULT_LOOP_DELAY: u64 = 1000;

#[derive(Parser, Debug)]
#[command(name = "termtosvg")]
#[command(about = "Record a terminal session and render an SVG animation", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Program to record with optional arguments
    #[arg(short = 'c', long = "command", default_value_t = default_shell())]
    pub command_to_record: String,

    /// Duration between loops
    #[arg(short = 'D', long = "loop-delay", value_parser = integral_duration_validation, default_value_t = DEFAULT_LOOP_DELAY)]
    pub loop_delay: u64,

    /// Screen geometry (e.g. 82x24)
    #[arg(short = 'g', long = "screen-geometry")]
    pub screen_geometry: Option<String>,

    /// Minimum frame duration (ms)
    #[arg(short = 'm', long = "min-frame-duration", value_parser = integral_duration_validation, default_value_t = 1)]
    pub min_frame_duration: u64,

    /// Maximum frame duration (ms)
    #[arg(short = 'M', long = "max-frame-duration", value_parser = integral_duration_validation)]
    pub max_frame_duration: Option<u64>,

    /// Use still frames instead of animation
    #[arg(short = 's', long = "still-frames", default_value_t = false)]
    pub still_frames: bool,

    /// Template name or path
    #[arg(short = 't', long = "template")]
    pub template: Option<String>,

    /// Optional output path (positional or via -o/--output)
    #[arg(short = 'o', long = "output")]
    pub output_path: Option<String>,

    /// Optional positional output path
    pub output_path_pos: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Record the session to an asciicast file
    Record(RecordArgs),
    /// Render an existing asciicast recording
    Render(RenderArgs),
}

#[derive(Args, Debug)]
pub struct RecordArgs {
    /// Optional cast output path
    #[arg(short = 'o', long = "output")]
    pub output_path: Option<String>,

    /// Optional positional cast output path
    pub output_path_pos: Option<String>,

    /// Program to record with optional arguments
    #[arg(short = 'c', long = "command", default_value_t = default_shell())]
    pub command_to_record: String,

    /// Screen geometry (e.g. 82x24)
    #[arg(short = 'g', long = "screen-geometry")]
    pub screen_geometry: Option<String>,
}

#[derive(Args, Debug)]
pub struct RenderArgs {
    /// Input asciicast file
    pub input_file: String,

    /// Optional output path
    #[arg(short = 'o', long = "output")]
    pub output_path: Option<String>,

    /// Optional positional output path
    pub output_path_pos: Option<String>,

    /// Minimum frame duration (ms)
    #[arg(short = 'm', long = "min-frame-duration", value_parser = integral_duration_validation, default_value_t = 1)]
    pub min_frame_duration: u64,

    /// Maximum frame duration (ms)
    #[arg(short = 'M', long = "max-frame-duration", value_parser = integral_duration_validation)]
    pub max_frame_duration: Option<u64>,

    /// Use still frames instead of animation
    #[arg(short = 's', long = "still-frames", default_value_t = false)]
    pub still_frames: bool,

    /// Template name or path
    #[arg(short = 't', long = "template")]
    pub template: Option<String>,

    /// Duration between loops
    #[arg(short = 'D', long = "loop-delay", value_parser = integral_duration_validation, default_value_t = DEFAULT_LOOP_DELAY)]
    pub loop_delay: u64,
}

pub fn integral_duration_validation(value: &str) -> Result<u64, String> {
    let lower = value.to_lowercase();
    let numeric = lower.strip_suffix("ms").unwrap_or(&lower);
    numeric
        .parse::<u64>()
        .map_err(|_| "duration must be an integer greater than 0".to_string())
        .and_then(|v| {
            if v >= 1 {
                Ok(v)
            } else {
                Err("duration must be an integer greater than 0".to_string())
            }
        })
}

pub fn run(args: Vec<String>, input_fileno: RawFd, output_fileno: RawFd) -> Result<()> {
    let cli = Cli::parse_from(args);
    let templates = config::default_templates();
    let default_template = "powershell".to_string();
    match &cli.command {
        Some(Commands::Record(record_args)) => {
            let cast_filename = record_args
                .output_path
                .clone()
                .or_else(|| record_args.output_path_pos.clone())
                .unwrap_or_else(|| temp_cast_file());
            let process_args = parse_process_args(&record_args.command_to_record);
            let geometry =
                geometry_or_default(record_args.screen_geometry.as_deref(), output_fileno)?;
            record_subcommand(
                process_args,
                geometry,
                input_fileno,
                output_fileno,
                &cast_filename,
            )?;
            println!("cast file created at {cast_filename}");
        }
        Some(Commands::Render(render_args)) => {
            let template_name = render_args
                .template
                .clone()
                .unwrap_or_else(|| default_template.clone());
            let template_bytes = anim::validate_template(&template_name, &templates)?;
            let output_path = render_args
                .output_path
                .clone()
                .or_else(|| render_args.output_path_pos.clone())
                .unwrap_or_else(|| default_output_path(render_args.still_frames));
            render_subcommand(
                render_args.still_frames,
                template_bytes.as_slice(),
                &render_args.input_file,
                &output_path,
                render_args.min_frame_duration,
                render_args.max_frame_duration,
                render_args.loop_delay,
            )?;
            println!("rendered to {output_path}");
        }
        None => {
            // record and render on the fly
            eprintln!("Recording started, press Ctrl-D to finish");
            let template_name = cli.template.clone().unwrap_or(default_template);
            let template_bytes = anim::validate_template(&template_name, &templates)?;
            let output_path = cli
                .output_path
                .clone()
                .or_else(|| cli.output_path_pos.clone())
                .unwrap_or_else(|| default_output_path(cli.still_frames));
            let geometry = geometry_or_default(cli.screen_geometry.as_deref(), output_fileno)?;
            let process_args = parse_process_args(&cli.command_to_record);
            record_render_subcommand(
                process_args,
                cli.still_frames,
                template_bytes.as_slice(),
                geometry,
                input_fileno,
                output_fileno,
                &output_path,
                cli.min_frame_duration,
                cli.max_frame_duration,
                cli.loop_delay,
            )?;
            println!("rendered to {output_path}");
        }
    }

    Ok(())
}

fn parse_process_args(command: &str) -> Vec<String> {
    shlex::Shlex::new(command).collect()
}

fn geometry_or_default(geometry: Option<&str>, fileno: RawFd) -> Result<(u16, u16)> {
    if let Some(g) = geometry {
        return config::validate_geometry(g).map_err(anyhow::Error::msg);
    }
    Ok(term::get_terminal_size(fileno))
}

fn record_subcommand(
    process_args: Vec<String>,
    geometry: (u16, u16),
    input_fileno: RawFd,
    output_fileno: RawFd,
    cast_filename: &str,
) -> Result<()> {
    eprintln!("Recording started, press Ctrl-D to finish");
    let records = term::record(
        &process_args,
        geometry.0,
        geometry.1,
        input_fileno,
        output_fileno,
    );
    let mut file = File::create(cast_filename)?;
    for record in records {
        let line = match record {
            AsciiCastV2Record::Header(h) => h.to_json_line()?,
            AsciiCastV2Record::Event(e) => e.to_json_line()?,
        };
        writeln!(file, "{}", line)?;
    }
    Ok(())
}

fn render_subcommand(
    still: bool,
    template: &[u8],
    cast_filename: &str,
    output_path: &str,
    min_frame_duration: u64,
    max_frame_duration: Option<u64>,
    loop_delay: u64,
) -> Result<()> {
    let records = asciicast::read_records(cast_filename)?;
    let (geometry, frames_iter) =
        term::timed_frames(records, min_frame_duration, max_frame_duration, loop_delay);
    if still {
        anim::render_still_frames(
            frames_iter.collect::<Vec<TimedFrame>>(),
            geometry,
            output_path,
            template,
        )?;
    } else {
        anim::render_animation(frames_iter, geometry, output_path, template)?;
    }
    Ok(())
}

fn record_render_subcommand(
    process_args: Vec<String>,
    still: bool,
    template: &[u8],
    geometry: (u16, u16),
    input_fileno: RawFd,
    output_fileno: RawFd,
    output_path: &str,
    min_frame_duration: u64,
    max_frame_duration: Option<u64>,
    loop_delay: u64,
) -> Result<()> {
    let records = term::record(
        &process_args,
        geometry.0,
        geometry.1,
        input_fileno,
        output_fileno,
    );
    let (geometry, frames_iter) =
        term::timed_frames(records, min_frame_duration, max_frame_duration, loop_delay);
    if still {
        anim::render_still_frames(
            frames_iter.collect::<Vec<TimedFrame>>(),
            geometry,
            output_path,
            template,
        )?;
    } else {
        anim::render_animation(frames_iter, geometry, output_path, template)?;
    }
    Ok(())
}

fn default_output_path(still_frames: bool) -> String {
    if still_frames {
        temp_still_dir().expect("tempdir")
    } else {
        NamedTempFile::new()
            .expect("temp file")
            .into_temp_path()
            .to_path_buf()
            .display()
            .to_string()
    }
}

fn temp_cast_file() -> String {
    NamedTempFile::new()
        .expect("temp cast")
        .into_temp_path()
        .with_extension("cast")
        .to_path_buf()
        .display()
        .to_string()
}

fn temp_still_dir() -> Result<String> {
    let rand_suffix: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(10)
        .map(char::from)
        .collect();
    let path = std::env::temp_dir().join(format!("termtosvg_{rand_suffix}"));
    std::fs::create_dir_all(&path)?;
    Ok(path.display().to_string())
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "sh".into())
}
