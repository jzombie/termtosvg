use std::fs::File;
use std::io::Write;
use std::os::fd::BorrowedFd;
use std::os::unix::io::RawFd;

use anyhow::Result;
use clap::{Args, Command, CommandFactory, FromArgMatches, Parser, Subcommand};
use indoc::indoc;
use nix::unistd::isatty;
use rand::{Rng, distr::Alphanumeric};
use tempfile::NamedTempFile;

use crate::anim;
use crate::asciicast::{self, AsciiCastV2Event, AsciiCastV2Header, AsciiCastV2Record};
use crate::config;
use crate::term::{self, TimedFrame};

pub const DEFAULT_LOOP_DELAY: u64 = 1000;

#[derive(Parser, Debug)]
#[command(name = "termtosvg")]
#[command(
        about = "Record a terminal session and render an SVG animation",
        long_about = indoc!(r#"
                Record a terminal session and render an SVG animation.

                Stdin behavior:

                - When run with the `render` subcommand, `-` may be used as the input filename
                    to read an asciicast recording from stdin (v2 lines or v1 JSON).
                - When the program is invoked with no subcommand and stdin is a pipe, the
                    stdin bytes are treated as raw terminal output and rendered as a single
                    `o` event. To force parsing stdin as an asciicast, use `render -`.

                Examples:

                    # Render a cast file on disk
                    termtosvg render demo.cast -o out.svg

                    # Render an asciicast streamed to stdin
                    cat demo.cast | termtosvg render - -o piped.svg

                    # Pipe raw terminal output (ANSI sequences preserved) to the default mode
                    neofetch | termtosvg -g 82x24 -o neofetch.svg
        "#),
)]
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
    // Integrate template names into clap's help output by rewriting the
    // `template` argument help text before parsing CLI arguments.
    let templates = config::default_templates();
    let mut names: Vec<&str> = templates.keys().map(|s| s.as_str()).collect();
    names.sort();
    let template_help = format!(
        "Template name or path. Built-in templates: {}",
        names.join(", ")
    );
    let template_help: &'static str = Box::leak(template_help.into_boxed_str());

    let mut cmd = Cli::command();
    cmd = annotate_template_help(cmd, template_help);
    let matches = cmd.try_get_matches_from(&args).unwrap_or_else(|e| e.exit());
    let cli =
        Cli::from_arg_matches(&matches).map_err(|e: clap::Error| anyhow::anyhow!(e.to_string()))?;
    let templates = config::default_templates();
    let default_template = "powershell".to_string();
    match &cli.command {
        Some(Commands::Record(record_args)) => {
            let cast_filename = record_args
                .output_path
                .clone()
                .or_else(|| record_args.output_path_pos.clone())
                .unwrap_or_else(temp_cast_file);
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
            let stdin_is_tty =
                unsafe { isatty(BorrowedFd::borrow_raw(input_fileno)).unwrap_or(true) };
            let template_name = cli.template.clone().unwrap_or(default_template);
            let template_bytes = anim::validate_template(&template_name, &templates)?;
            let output_path = cli
                .output_path
                .clone()
                .or_else(|| cli.output_path_pos.clone())
                .unwrap_or_else(|| default_output_path(cli.still_frames));
            let geometry = geometry_or_default(cli.screen_geometry.as_deref(), output_fileno)?;
            if stdin_is_tty {
                eprintln!("Recording started, press Ctrl-D to finish");
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
            } else {
                use std::io::Read;

                let mut stdin = std::io::stdin();
                let mut buf = String::new();
                stdin.read_to_string(&mut buf)?;
                if buf.is_empty() {
                    anyhow::bail!("No data received on stdin to render");
                }

                // If stdin contains a full asciicast (v2 lines or v1), parse
                // and render it as such. Otherwise treat the input as raw
                // terminal output and render a single event containing the
                // bytes read.
                match asciicast::parse_records_from_str(&buf) {
                    Ok(records) if !records.is_empty() => {
                        render_records(
                            cli.still_frames,
                            template_bytes.as_slice(),
                            records,
                            &output_path,
                            cli.min_frame_duration,
                            cli.max_frame_duration,
                            cli.loop_delay,
                        )?;
                    }
                    _ => {
                        let records = vec![
                            AsciiCastV2Record::Header(AsciiCastV2Header::new(
                                2, geometry.0, geometry.1, None, None,
                            )?),
                            AsciiCastV2Record::Event(AsciiCastV2Event::new(0.0, "o", &buf, None)?),
                        ];
                        render_records(
                            cli.still_frames,
                            template_bytes.as_slice(),
                            records,
                            &output_path,
                            cli.min_frame_duration,
                            cli.max_frame_duration,
                            cli.loop_delay,
                        )?;
                    }
                }
            }
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
    render_records(
        still,
        template,
        records,
        output_path,
        min_frame_duration,
        max_frame_duration,
        loop_delay,
    )
}

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
fn render_records(
    still: bool,
    template: &[u8],
    records: Vec<AsciiCastV2Record>,
    output_path: &str,
    min_frame_duration: u64,
    max_frame_duration: Option<u64>,
    loop_delay: u64,
) -> Result<()> {
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

#[allow(clippy::too_many_arguments)]
fn render_stdin_stream(
    still: bool,
    template: &[u8],
    geometry: (u16, u16),
    output_path: &str,
    min_frame_duration: u64,
    max_frame_duration: Option<u64>,
    loop_delay: u64,
) -> Result<()> {
    use std::io::Read;

    let mut stdin = std::io::stdin();
    let mut buf = String::new();
    stdin.read_to_string(&mut buf)?;
    if buf.is_empty() {
        anyhow::bail!("No data received on stdin to render");
    }

    let records = vec![
        AsciiCastV2Record::Header(AsciiCastV2Header::new(
            2, geometry.0, geometry.1, None, None,
        )?),
        AsciiCastV2Record::Event(AsciiCastV2Event::new(0.0, "o", &buf, None)?),
    ];

    render_records(
        still,
        template,
        records,
        output_path,
        min_frame_duration,
        max_frame_duration,
        loop_delay,
    )
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

fn annotate_template_help(mut command: Command, help: &'static str) -> Command {
    let has_template_arg = command
        .get_arguments()
        .any(|arg| arg.get_id() == "template");
    if has_template_arg {
        command = command.mut_arg("template", |arg| arg.help(help).long_help(help));
    }

    let sub_names: Vec<String> = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();
    for name in sub_names {
        if let Some(sub) = command.find_subcommand_mut(&name) {
            let sub_owned = std::mem::take(sub);
            *sub = annotate_template_help(sub_owned, help);
        }
    }
    command
}
