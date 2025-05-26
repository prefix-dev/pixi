use super::PtyProcess;
use crate::unix::pty_process::PtyProcessOptions;
use libc::SIGWINCH;
use nix::sys::select::FdSet;
use nix::{
    errno::Errno,
    sys::{select, time::TimeVal, wait::WaitStatus},
};
use signal_hook::iterator::Signals;
use std::time::{Duration, Instant};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::AsFd,
    process::Command,
};

pub struct PtySession {
    pub process: PtyProcess,

    /// A file handle of the stdout of the pty process
    pub process_stdout: File,

    /// A file handle of the stdin of the pty process
    pub process_stdin: File,

    /// Rolling buffer used for the wait_until pattern matching
    pub rolling_buffer: Vec<u8>,
}

/// ```
/// use std::process::Command;
/// use pixi_pty::unix::PtySession;
///
/// let process = PtySession::new(Command::new("bash")).unwrap();
/// ```
impl PtySession {
    /// Constructs a new session
    pub fn new(command: Command) -> io::Result<Self> {
        let process = PtyProcess::new(
            command,
            PtyProcessOptions {
                echo: true,
                ..Default::default()
            },
        )?;

        let process_stdin = process.get_file_handle()?;
        let process_stdout = process.get_file_handle()?;

        Ok(Self {
            process,
            process_stdout,
            process_stdin,
            rolling_buffer: Vec::with_capacity(4096),
        })
    }

    /// Send string to process. As stdin of the process is most likely buffered, you'd
    /// need to call `flush()` after `send()` to make the process actually see your input.
    ///
    /// Returns number of written bytes
    pub fn send<B: AsRef<[u8]>>(&mut self, s: B) -> io::Result<usize> {
        // sleep for 0.05 seconds to delay sending the next command
        std::thread::sleep(Duration::from_millis(50));
        self.process_stdin.write(s.as_ref())
    }

    /// Sends string and a newline to process. This is guaranteed to be flushed to the process.
    /// Returns number of written bytes.
    pub fn send_line(&mut self, line: &str) -> io::Result<usize> {
        let result = self.send(format!("{line}\n"))?;
        self.flush()?;
        Ok(result)
    }

    /// Make sure all bytes written via `send()` are sent to the process
    pub fn flush(&mut self) -> io::Result<()> {
        self.process_stdin.flush()
    }

    /// Interact with the process. This will put the current process into raw mode and
    /// forward all input from stdin to the process and all output from the process to stdout.
    /// This will block until the process exits.
    pub fn interact(&mut self, wait_until: Option<&str>) -> io::Result<Option<i32>> {
        let pattern_timeout = Duration::from_secs(3);
        let pattern_start = Instant::now();

        // Make sure anything we have written so far has been flushed.
        self.flush()?;

        // Put the process into raw mode
        let original_mode = self.process.set_raw()?;

        let process_stdout_clone = self.process_stdout.try_clone()?;
        let process_stdout_fd = process_stdout_clone.as_fd();
        let stdin = std::io::stdin();
        let stdin_fd = stdin.as_fd();

        // Create a FDSet for the select call
        let mut fd_set = FdSet::new();
        fd_set.insert(process_stdout_fd);
        fd_set.insert(stdin_fd);

        // Create a buffer for reading from the process
        let mut buf = [0u8; 4096];

        // Catch the SIGWINCH signal to handle window resizing
        // and forward the new terminal size to the process
        let mut signals = Signals::new([SIGWINCH])?;

        let mut write_stdout = wait_until.is_none();
        // Call select in a loop and handle incoming data
        let exit_status = loop {
            // Make sure that the process is still alive
            let status = self.process.status();
            if status != Some(WaitStatus::StillAlive) {
                break status;
            }

            // Handle window resizing
            for signal in signals.pending() {
                match signal {
                    SIGWINCH => {
                        // get window size
                        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
                        let res = unsafe {
                            libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size)
                        };
                        if res == 0 {
                            self.process.set_window_size(size)?;
                        }
                    }
                    _ => unreachable!(),
                }
            }

            // Check if we have waited long enough for the pattern
            if pattern_start.elapsed() > pattern_timeout && !write_stdout {
                io::stdout().write_all(
                    format!(
                        "WARNING: Did not detect successful shell initialization within {} second(s).\n\r         Please check on https://pixi.sh/latest/advanced/pixi_shell/#issues-with-pixi-shell for more tips.\n\r",
                        pattern_timeout.as_secs()
                    )
                    .as_bytes(),
                )?;
                io::stdout().write_all(&self.rolling_buffer)?;
                io::stdout().flush()?;
                write_stdout = true;
            }

            let mut select_timeout = TimeVal::new(0, 100_000);
            let mut select_set = fd_set;

            let res = select::select(None, &mut select_set, None, None, &mut select_timeout);
            if let Err(error) = res {
                if error == Errno::EINTR {
                    // EINTR is not an error, it just means that we got interrupted by a signal (e.g. SIGWINCH)
                    continue;
                } else {
                    self.process.set_mode(original_mode)?;
                    return Err(std::io::Error::from(error));
                }
            } else {
                // We have new data coming from the process
                if select_set.contains(process_stdout_fd) {
                    let bytes_read = self.process_stdout.read(&mut buf).unwrap_or(0);
                    if !write_stdout {
                        if let Some(wait_until) = wait_until {
                            // Append new data to rolling buffer
                            self.rolling_buffer.extend_from_slice(&buf[..bytes_read]);

                            // Find the first occurrence of the pattern
                            if let Some(window_pos) = self
                                .rolling_buffer
                                .windows(wait_until.len())
                                .position(|window| window == wait_until.as_bytes())
                            {
                                write_stdout = true;

                                // Calculate position after the pattern
                                let output_start = window_pos + wait_until.len();

                                // Write remaining buffered content after the pattern
                                if output_start < self.rolling_buffer.len() {
                                    io::stdout().write_all(&self.rolling_buffer[output_start..])?;
                                    io::stdout().flush()?;
                                }

                                // Clear the rolling buffer as we don't need it anymore
                                self.rolling_buffer.clear();
                            } else {
                                // Keep only up to 2 * wait_until.len() bytes from the end
                                // This ensures we don't miss matches across buffer boundaries
                                let keep_size = wait_until.len() * 2;
                                if self.rolling_buffer.len() > keep_size {
                                    self.rolling_buffer = self
                                        .rolling_buffer
                                        .split_off(self.rolling_buffer.len() - keep_size);
                                }
                            }
                        }
                    } else if bytes_read > 0 {
                        io::stdout().write_all(&buf[..bytes_read])?;
                        io::stdout().flush()?;
                    }
                }

                // or from stdin
                if select_set.contains(stdin_fd) {
                    let bytes_read = io::stdin().read(&mut buf)?;
                    self.process_stdin.write_all(&buf[..bytes_read])?;
                    self.process_stdin.flush()?;
                }
            }
        };

        // Restore the original terminal mode
        self.process.set_mode(original_mode)?;

        match exit_status {
            Some(WaitStatus::Exited(_, code)) => Ok(Some(code)),
            Some(WaitStatus::Signaled(_, signal, _)) => Ok(Some(128 + signal as i32)),
            _ => Ok(None),
        }
    }
}
