use super::PtyProcess;
use crate::unix::pty_process::PtyProcessOptions;
use libc::SIGWINCH;
use nix::sys::select::FdSet;
use nix::{
    errno::Errno,
    sys::{select, time::TimeVal, wait::WaitStatus},
};
use signal_hook::iterator::Signals;
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

        let process_stdin = process.get_file_handle()?;
        let process_stdout = process.get_file_handle()?;

        Ok(Self {
            process,
            process_stdout,
            process_stdin,
        })
    }

    /// Send string to process. As stdin of the process is most likely buffered, you'd
    /// need to call `flush()` after `send()` to make the process actually see your input.
    ///
    /// Returns number of written bytes
    pub fn send<B: AsRef<[u8]>>(&mut self, s: B) -> io::Result<usize> {
        self.process_stdin.write(s.as_ref())
    }

    /// Sends string and a newline to process. This is guaranteed to be flushed to the process.
    /// Returns number of written bytes.
    pub fn send_line(&mut self, line: &str) -> io::Result<usize> {
        let mut len = self.send(line)?;
        len += self.process_stdin.write(&[b'\n'])?;
        Ok(len)
    }

    /// Make sure all bytes written via `send()` are sent to the process
    pub fn flush(&mut self) -> io::Result<()> {
        self.process_stdin.flush()
    }

    /// Interact with the process. This will put the current process into raw mode and
    /// forward all input from stdin to the process and all output from the process to stdout.
    /// This will block until the process exits.
    pub fn interact(&mut self) -> io::Result<Option<i32>> {
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
        fd_set.insert(&process_stdout_fd);
        fd_set.insert(&stdin_fd);

        // Create a buffer for reading from the process
        let mut buf = [0u8; 2048];

        // Catch the SIGWINCH signal to handle window resizing
        // and forward the new terminal size to the process
        let mut signals = Signals::new([SIGWINCH])?;

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

            let mut select_timeout = TimeVal::new(4, 0);
            let mut select_set = fd_set;

            let res = select::select(None, &mut select_set, None, None, &mut select_timeout);
            if res.is_err() {
                if let Err(Errno::EINTR) = res {
                    // EINTR is not an error, it just means that we got interrupted by a signal (e.g. SIGWINCH)
                    continue;
                } else {
                    self.process.set_mode(original_mode)?;
                    return Err(std::io::Error::from(res.unwrap_err()));
                }
            } else {
                // We have new data coming from the process
                if select_set.contains(&process_stdout_fd) {
                    let bytes_read = self.process_stdout.read(&mut buf)?;
                    std::io::stdout().write_all(&buf[..bytes_read])?;
                    std::io::stdout().flush()?;
                }

                // or from stdin
                if select_set.contains(&stdin_fd) {
                    let bytes_read = std::io::stdin().read(&mut buf)?;
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
