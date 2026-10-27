/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#[cfg(unix)]
use std::fs::File;
use std::io;
use std::io::Write;
#[cfg(unix)]
use std::sync::OnceLock;

pub enum Color {
    Red,
    Green,
}

impl Color {
    fn ansi_sequence(self) -> &'static str {
        match self {
            Color::Red => "\x1b[31m",
            Color::Green => "\x1b[32m",
        }
    }
}

pub fn clear() {
    if write_ansi("\x1b[2J").is_err() {
        print!("{}", "\n".repeat(8));
    }
}

pub fn fg(color: Color) {
    let _ = write_ansi(color.ansi_sequence());
}

pub fn reset() {
    let _ = write_ansi("\x1b[39m");
}

fn write_ansi(sequence: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

#[cfg(unix)]
static TERMINAL_SIZE: OnceLock<Option<(usize, usize)>> = OnceLock::new();

#[cfg(unix)]
pub fn size() -> Option<(usize, usize)> {
    *TERMINAL_SIZE.get_or_init(|| {
        let tty = File::open("/dev/tty").ok()?;
        let size = rustix::termios::tcgetwinsize(&tty).ok()?;
        Some((usize::from(size.ws_col), usize::from(size.ws_row)))
    })
}

#[cfg(not(unix))]
pub fn size() -> Option<(usize, usize)> {
    None
}
