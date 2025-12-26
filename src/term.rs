use std::collections::BTreeMap;
use std::os::unix::io::{BorrowedFd, IntoRawFd, RawFd};
use std::time::Instant;

use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::libc;
use nix::poll::{poll, PollFd, PollFlags};
use nix::pty::{openpty, Winsize};
use nix::sys::termios::{self, SetArg, Termios};
use nix::sys::wait::waitpid;
use nix::unistd::{close, dup2, execvp, fork, read, write, ForkResult, Pid};
use vte::{Params, Parser, Perform};
use unicode_width::UnicodeWidthChar;

use crate::anim::CharacterCell;
use crate::asciicast::{AsciiCastV2Event, AsciiCastV2Header, AsciiCastV2Record};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimedFrame {
    pub time: u64,
    pub duration: u64,
    pub buffer: BTreeMap<usize, BTreeMap<usize, CharacterCell>>,
}

#[derive(Debug, Clone)]
pub struct TerminalMode {
    fileno: RawFd,
    original_termios: Option<Termios>,
    original_winsize: Option<Winsize>,
}

impl TerminalMode {
    pub fn new(fileno: RawFd) -> Self {
        TerminalMode {
            fileno,
            original_termios: None,
            original_winsize: None,
        }
    }

    pub fn enter(&mut self) {
        if let Ok(mut t) = termios::tcgetattr(unsafe { BorrowedFd::borrow_raw(self.fileno) }) {
            self.original_termios = Some(t.clone());
            termios::cfmakeraw(&mut t);
            let _ = termios::tcsetattr(unsafe { BorrowedFd::borrow_raw(self.fileno) }, SetArg::TCSANOW, &t);
        }

        // Save window size
        let mut ws: Winsize = Winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            if libc::ioctl(self.fileno, libc::TIOCGWINSZ, &mut ws) == 0 {
                self.original_winsize = Some(ws);
            }
        }
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        if let Some(orig) = &self.original_termios {
            let _ = termios::tcsetattr(unsafe { BorrowedFd::borrow_raw(self.fileno) }, SetArg::TCSANOW, orig);
        }
        if let Some(ws) = &self.original_winsize {
            unsafe {
                let _ = libc::ioctl(self.fileno, libc::TIOCSWINSZ, ws as *const _);
            }
        }
    }
}

pub fn get_terminal_size(fileno: RawFd) -> (u16, u16) {
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fileno, libc::TIOCGWINSZ, &mut size) == 0 {
            if size.ws_col > 0 && size.ws_row > 0 {
                return (size.ws_col, size.ws_row);
            }
        }
    }
    (80, 24)
}

fn spawn_pty(process_args: &[String], columns: u16, lines: u16) -> nix::Result<(RawFd, Pid)> {
    let winsize = Winsize {
        ws_row: lines,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let pty = openpty(Some(&winsize), None)?;
    let master_fd = pty.master.into_raw_fd();
    let slave_fd = pty.slave.into_raw_fd();

    match unsafe { fork()? } {
        ForkResult::Child => {
            // Create new session and set controlling terminal
            let _ = nix::unistd::setsid();
            let _ = unsafe { libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0) };
            // Redirect stdio to slave
            let _ = dup2(slave_fd, libc::STDIN_FILENO);
            let _ = dup2(slave_fd, libc::STDOUT_FILENO);
            let _ = dup2(slave_fd, libc::STDERR_FILENO);
            let _ = close(master_fd);
            let _ = close(slave_fd);

            if !process_args.is_empty() {
                let prog = std::ffi::CString::new(process_args[0].as_str()).unwrap();
                let argv: Vec<std::ffi::CString> = process_args
                    .iter()
                    .map(|s| std::ffi::CString::new(s.as_str()).unwrap())
                    .collect();
                let _ = execvp(&prog, &argv);
            }
            std::process::exit(1);
        }
        ForkResult::Parent { child } => {
            let _ = close(slave_fd);
            Ok((master_fd, child))
        }
    }
}

pub fn record(process_args: &[String], columns: u16, lines: u16, input_fileno: RawFd, output_fileno: RawFd) -> Vec<AsciiCastV2Record> {
    let mut guard = TerminalMode::new(input_fileno);
    guard.enter();

    let mut records = Vec::new();
    records.push(AsciiCastV2Record::Header(
        AsciiCastV2Header::new(2, columns, lines, None, None).expect("header"),
    ));

    let (master_fd, child_pid) = match spawn_pty(process_args, columns, lines) {
        Ok(v) => v,
        Err(_) => return records,
    };

    // Make master non-blocking
    let _ = fcntl(master_fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK));

    let mut start = None;
    let mut running = true;
    let mut buf = [0u8; 1024];

    let input_fd = unsafe { BorrowedFd::borrow_raw(input_fileno) };
    let master_fd_borrowed = unsafe { BorrowedFd::borrow_raw(master_fd) };

    while running {
        let mut fds = [
            PollFd::new(&input_fd, PollFlags::POLLIN),
            PollFd::new(&master_fd_borrowed, PollFlags::POLLIN),
        ];

        let _ = poll(&mut fds, 100);

        // stdin -> pty
        if let Some(revents) = fds[0].revents() {
            if revents.contains(PollFlags::POLLIN) {
                match read(input_fileno, &mut buf) {
                    Ok(0) => running = false,
                    Ok(n) => {
                        let _ = write(master_fd, &buf[..n]);
                    }
                    Err(err) => {
                        if err != nix::errno::Errno::EAGAIN {
                            running = false;
                        }
                    }
                }
            }
        }

        // pty -> stdout + records
        if let Some(revents) = fds[1].revents() {
            if revents.contains(PollFlags::POLLIN) {
                match read(master_fd, &mut buf) {
                    Ok(0) => running = false,
                    Ok(n) => {
                        let now = Instant::now();
                        if start.is_none() {
                            start = Some(now);
                        }
                        let elapsed = start.map(|s| now.duration_since(s).as_secs_f64()).unwrap_or(0.0);
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        let _ = write(output_fileno, &buf[..n]);
                        records.push(AsciiCastV2Record::Event(
                            AsciiCastV2Event::new(elapsed, "o", &text, None).expect("event"),
                        ));
                    }
                    Err(err) => {
                        if err != nix::errno::Errno::EAGAIN {
                            running = false;
                        }
                    }
                }
            }
        }
    }

    let _ = close(master_fd);
    let _ = waitpid(child_pid, None);
    records
}

pub fn _group_by_time(
    event_records: impl IntoIterator<Item = AsciiCastV2Event>,
    min_rec_duration: u64,
    max_rec_duration: Option<u64>,
    last_rec_duration: u64,
) -> Vec<AsciiCastV2Event> {
    let mut grouped = Vec::new();
    let mut current_string = String::new();
    let mut current_time = 0.0_f64;
    let mut dropped_time = 0.0_f64;
    let max_rec_duration = max_rec_duration.map(|m| m as f64 / 1000.0);

    for event_record in event_records {
        if event_record.event_type != "o" {
            continue;
        }
        let time_between_events = event_record.time - (current_time + dropped_time);
        if time_between_events * 1000.0 >= min_rec_duration as f64 {
            let mut duration = time_between_events;
            if let Some(max_rec) = max_rec_duration {
                if duration > max_rec {
                    dropped_time += duration - max_rec;
                    duration = max_rec;
                }
            }
            grouped.push(
                AsciiCastV2Event::new(current_time, "o", &current_string, Some(duration))
                    .expect("event"),
            );
            current_string.clear();
            current_time += duration;
        }
        current_string.push_str(&event_record.event_data);
    }

    grouped.push(
        AsciiCastV2Event::new(
            current_time,
            "o",
            &current_string,
            Some(last_rec_duration as f64 / 1000.0),
        )
        .expect("event"),
    );
    grouped
}

#[derive(Clone)]
struct CellAttributes {
    fg: String,
    bg: String,
    bold: bool,
    italics: bool,
    underline: bool,
    strikethrough: bool,
    inverse: bool,
}

impl CellAttributes {
    fn default() -> Self {
        CellAttributes {
            fg: "foreground".into(),
            bg: "background".into(),
            bold: false,
            italics: false,
            underline: false,
            strikethrough: false,
            inverse: false,
        }
    }

    fn reset(&mut self) {
        *self = CellAttributes::default();
    }

    fn set_sgr(&mut self, params: &[i64]) {
        if params.is_empty() {
            self.reset();
            return;
        }
        for &p in params {
            match p {
                0 => self.reset(),
                1 => self.bold = true,
                3 => self.italics = true,
                4 => self.underline = true,
                7 => self.inverse = true,
                9 => self.strikethrough = true,
                22 => self.bold = false,
                23 => self.italics = false,
                24 => self.underline = false,
                27 => self.inverse = false,
                29 => self.strikethrough = false,
                30..=37 => self.fg = ansi_color((p - 30) as u8, false),
                90..=97 => self.fg = ansi_color((p - 90) as u8, true),
                39 => self.fg = "foreground".into(),
                40..=47 => self.bg = ansi_color((p - 40) as u8, false),
                100..=107 => self.bg = ansi_color((p - 100) as u8, true),
                49 => self.bg = "background".into(),
                _ => {}
            }
        }
    }

    fn effective_colors(&self) -> (String, String) {
        if self.inverse {
            (self.bg.clone(), self.fg.clone())
        } else {
            (self.fg.clone(), self.bg.clone())
        }
    }
}

fn ansi_color(idx: u8, bright: bool) -> String {
    // Basic ANSI palette
    const BASE: [(u8, u8, u8); 8] = [
        (0x00, 0x00, 0x00), // black
        (0xcd, 0x00, 0x00), // red
        (0x00, 0xcd, 0x00), // green
        (0xcd, 0xcd, 0x00), // yellow
        (0x00, 0x00, 0xee), // blue
        (0xcd, 0x00, 0xcd), // magenta
        (0x00, 0xcd, 0xcd), // cyan
        (0xe5, 0xe5, 0xe5), // white
    ];
    let (r, g, b) = BASE[(idx as usize).min(7)];
    let (r, g, b) = if bright {
        (
            (r as u16 + 0x33).min(0xff) as u8,
            (g as u16 + 0x33).min(0xff) as u8,
            (b as u16 + 0x33).min(0xff) as u8,
        )
    } else {
        (r, g, b)
    };
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

struct TerminalEmulator {
    width: usize,
    height: usize,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: Option<(usize, usize)>,
    cells: Vec<Vec<Option<CharacterCell>>>,
    attr: CellAttributes,
}

impl TerminalEmulator {
    fn new(width: usize, height: usize) -> Self {
        TerminalEmulator {
            width,
            height,
            cursor_row: 0,
            cursor_col: 0,
            saved_cursor: None,
            cells: vec![vec![None; width]; height],
            attr: CellAttributes::default(),
        }
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row.min(self.height.saturating_sub(1));
        self.cursor_col = col.min(self.width.saturating_sub(1));
    }

    fn linefeed(&mut self) {
        if self.cursor_row + 1 >= self.height {
            self.scroll_up(1);
        } else {
            self.cursor_row += 1;
        }
    }

    fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    fn clear_screen(&mut self) {
        for row in &mut self.cells {
            for cell in row.iter_mut() {
                *cell = None;
            }
        }
        self.set_cursor(0, 0);
    }

    fn clear_to_screen_end(&mut self) {
        self.clear_line_from_cursor();
        for r in (self.cursor_row + 1)..self.height {
            for cell in self.cells[r].iter_mut() {
                *cell = None;
            }
        }
    }

    fn clear_line_from_cursor(&mut self) {
        if self.cursor_row < self.height {
            for cell in self.cells[self.cursor_row].iter_mut().skip(self.cursor_col) {
                *cell = None;
            }
        }
    }

    fn clear_line(&mut self) {
        if self.cursor_row < self.height {
            for cell in self.cells[self.cursor_row].iter_mut() {
                *cell = None;
            }
        }
    }

    fn scroll_up(&mut self, rows: usize) {
        let rows = rows.min(self.height);
        for _ in 0..rows {
            self.cells.remove(0);
            self.cells.push(vec![None; self.width]);
        }
        self.cursor_row = self.height.saturating_sub(1);
    }

    fn put_char(&mut self, ch: char) {
        let width = UnicodeWidthChar::width(ch).unwrap_or(1);
        if width == 0 {
            if self.cursor_col > 0 && self.cursor_row < self.height {
                let col = self.cursor_col - 1;
                if let Some(cell) = self.cells[self.cursor_row][col].as_mut() {
                    cell.text.push(ch);
                }
            }
            return;
        }

        if self.cursor_col >= self.width {
            self.linefeed();
            self.cursor_col = 0;
        }

        if self.cursor_row < self.height {
            let (fg, bg) = self.attr.effective_colors();
            let mut cell = CharacterCell::new(ch.to_string());
            cell.color = fg;
            cell.background_color = bg;
            cell.bold = self.attr.bold;
            cell.italics = self.attr.italics;
            cell.underscore = self.attr.underline;
            cell.strikethrough = self.attr.strikethrough;
            self.cells[self.cursor_row][self.cursor_col] = Some(cell);
        }

        self.cursor_col = (self.cursor_col + width).min(self.width);
        if self.cursor_col >= self.width {
            self.cursor_col = self.width.saturating_sub(1);
        }
    }

    fn move_up(&mut self, count: usize) {
        self.cursor_row = self.cursor_row.saturating_sub(count);
    }

    fn move_down(&mut self, count: usize) {
        self.cursor_row = (self.cursor_row + count).min(self.height.saturating_sub(1));
    }

    fn move_right(&mut self, count: usize) {
        self.cursor_col = (self.cursor_col + count).min(self.width.saturating_sub(1));
    }

    fn move_left(&mut self, count: usize) {
        self.cursor_col = self.cursor_col.saturating_sub(count);
    }

    fn buffer(&self) -> BTreeMap<usize, BTreeMap<usize, CharacterCell>> {
        let mut buffer = BTreeMap::new();
        for (row_idx, row) in self.cells.iter().enumerate() {
            let mut line = BTreeMap::new();
            for (col_idx, cell) in row.iter().enumerate() {
                if let Some(cell) = cell {
                    line.insert(col_idx, cell.clone());
                }
            }
            buffer.insert(row_idx, line);
        }
        // cursor with reverse colors
        let cursor_cell = CharacterCell {
            text: " ".into(),
            color: "background".into(),
            background_color: "foreground".into(),
            bold: false,
            italics: false,
            underscore: false,
            strikethrough: false,
        };
        buffer
            .entry(self.cursor_row)
            .or_default()
            .insert(self.cursor_col, cursor_cell);
        buffer
    }
}

impl Perform for TerminalEmulator {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | b'\x0b' | b'\x0c' => {
                self.linefeed();
                self.carriage_return();
            }
            b'\r' => self.carriage_return(),
            b'\x08' => self.backspace(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        let p = |idx: usize, default: i64| {
            params
                .iter()
                .nth(idx)
                .and_then(|v| v.first())
                .map(|v| *v as i64)
                .unwrap_or(default)
        };
        match action {
            'A' => self.move_up(p(0, 1) as usize),
            'B' => self.move_down(p(0, 1) as usize),
            'C' => self.move_right(p(0, 1) as usize),
            'D' => self.move_left(p(0, 1) as usize),
            'E' => {
                self.move_down(p(0, 1) as usize);
                self.carriage_return();
            }
            'F' => {
                self.move_up(p(0, 1) as usize);
                self.carriage_return();
            }
            'G' => {
                let col = p(0, 1).saturating_sub(1) as usize;
                self.set_cursor(self.cursor_row, col);
            }
            'H' | 'f' => {
                let row = p(0, 1).saturating_sub(1) as usize;
                let col = p(1, 1).saturating_sub(1) as usize;
                self.set_cursor(row, col);
            }
            'J' => match p(0, 0) {
                0 => self.clear_to_screen_end(),
                2 => self.clear_screen(),
                _ => {}
            },
            'K' => match p(0, 0) {
                0 => self.clear_line_from_cursor(),
                2 => self.clear_line(),
                _ => {}
            },
            'm' => {
                let vals: Vec<i64> = params
                    .iter()
                    .flat_map(|group| group.iter().map(|v| *v as i64))
                    .collect();
                self.attr.set_sgr(&vals);
            }
            's' => self.saved_cursor = Some((self.cursor_row, self.cursor_col)),
            'u' => {
                if let Some((r, c)) = self.saved_cursor {
                    self.set_cursor(r, c);
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'c' => {
                self.clear_screen();
                self.attr.reset();
            }
            _ => {}
        }
    }
}

pub fn timed_frames(
    records: impl IntoIterator<Item = AsciiCastV2Record>,
    min_frame_dur: u64,
    max_frame_dur: Option<u64>,
    last_frame_dur: u64,
) -> ((u16, u16), impl Iterator<Item = TimedFrame>) {
    let mut iter = records.into_iter();
    let header = iter.next().expect("header missing");
    let header = match header {
        AsciiCastV2Record::Header(h) => h,
        _ => panic!("expected header"),
    };

    let auto_max = header.idle_time_limit.map(|v| (v * 1000.0) as u64);
    let max_frame_dur = max_frame_dur.or(auto_max);

    let grouped_events = _group_by_time(
        iter.filter_map(|r| match r {
            AsciiCastV2Record::Event(e) => Some(e),
            _ => None,
        }),
        min_frame_dur,
        max_frame_dur,
        last_frame_dur,
    );

    let width = header.width as usize;
    let height = header.height as usize;
    let mut term = TerminalEmulator::new(width, height);
    let mut parser = Parser::new();

    let frames_iter = grouped_events.into_iter().map(move |event| {
        for byte in event.event_data.as_bytes() {
            parser.advance(&mut term, *byte);
        }
        TimedFrame {
            time: (event.time * 1000.0) as u64,
            duration: (event.duration.unwrap_or(0.0) * 1000.0) as u64,
            buffer: term.buffer(),
        }
    });

    ((header.width, header.height), frames_iter)
}
