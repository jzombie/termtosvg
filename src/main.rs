use std::os::unix::io::AsRawFd;

use anyhow::Result;

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    let stdin_fd = std::io::stdin().as_raw_fd();
    let stdout_fd = std::io::stdout().as_raw_fd();
    termtosvg::cli::run(args, stdin_fd, stdout_fd)
}
