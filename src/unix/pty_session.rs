use super::PtyProcess;
use crate::unix::pty_process::PtyProcessOptions;
use nix::sys::wait::WaitStatus;
use std::io;
use std::io::Write;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::process::Command;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::signal::unix::SignalKind;

pub struct PtySession {
    pub process: PtyProcess,

    /// A file handle of the stdout of the pty process
    pub process_stdout: tokio::fs::File,

    /// A file handle of the stdin of the pty process
    pub process_stdin: tokio::fs::File,
}

/// ```
/// use std::process::Command;
/// use pixi::unix::PtySession;
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
        let process_stdout = process.get_file_handle()?;
        let process_stdin = process_stdout.try_clone()?;
        Ok(Self {
            process,
            process_stdout: File::from_std(process_stdout),
            process_stdin: File::from_std(process_stdin),
        })
    }

    /// Send string to process. As stdin of the process is most likely buffered, you'd
    /// need to call `flush()` after `send()` to make the process actually see your input.
    ///
    /// Returns number of written bytes
    pub async fn send<B: AsRef<[u8]>>(&mut self, s: B) -> io::Result<usize> {
        self.process_stdin.write(s.as_ref()).await
    }

    /// Sends string and a newline to process. This is guaranteed to be flushed to the process.
    /// Returns number of written bytes.
    pub async fn send_line(&mut self, line: &str) -> io::Result<usize> {
        let mut len = self.send(line).await?;
        len += self.process_stdin.write(&[b'\n']).await?;
        Ok(len)
    }

    /// Make sure all bytes written via `send()` are sent to the process
    pub async fn flush(&mut self) -> io::Result<()> {
        self.process_stdin.flush().await
    }

    pub async fn interact(&mut self) -> io::Result<()> {
        // Make sure anything we have written so far has been flushed.
        self.flush().await?;

        // Put the process into raw mode
        let original_mode = self.process.set_raw()?;

        // Bind to the SIGWINCH signal
        let mut signal_window_change = tokio::signal::unix::signal(SignalKind::window_change())?;

        let mut stdout_bytes = vec![0u8; 8096];
        let mut stdin_bytes = vec![0u8; 8096];

        let mut parent_stdin = tokio::io::stdin();
        while self.process.status() == Some(WaitStatus::StillAlive) {
            tokio::select! {
                // Forward any output from stdout to this processes stdout
                bytes_read = self.process_stdout.read(&mut stdout_bytes) => {
                    tokio::io::stdout().write_all(&stdout_bytes).await?;
                    tokio::io::stdout().flush().await?;
                }

                // Forward any input from this process to the pty process
                bytes_read = parent_stdin.read(&mut stdin_bytes) => {
                    self.process_stdin.write_all(&stdin_bytes).await?;
                    self.process_stdin.flush().await?;
                }

                // If the window size changed we also forward that to the sub-process
                signal = signal_window_change.recv() => {
                    // Query the terminal dimensions
                    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
                    let res = unsafe { libc::ioctl(io::stdout().as_raw_fd(), nix::libc::TIOCSWINSZ, &mut size) };
                    if res == 0 {
                        self.process.set_window_size(size)?;
                    }
                }
            }
        }

        self.process.set_mode(original_mode)?;

        Ok(())
    }
}
