use super::PtyProcess;
use crate::unix::pty_process::PtyProcessOptions;
use libc::EINTR;
use nix::sys::select::FdSet;
use nix::{
    errno::Errno,
    fcntl::{self, fcntl, FcntlArg, FdFlag},
    sys::{
        select,
        signal::{self, sigaction, SigAction, SigHandler, SigSet, Signal},
        termios::FlushArg,
        time::TimeVal,
        wait::WaitStatus,
    },
};
use std::os::fd::OwnedFd;
use std::{
    borrow::Borrow,
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd},
    process::Command,
};

use nix::unistd::{pipe, read, write};

pub struct PtySession {
    pub process: PtyProcess,

    /// A file handle of the stdout of the pty process
    pub process_stdout: File,

    /// A file handle of the stdin of the pty process
    pub process_stdin: File,
}


static mut SIGNAL_FD: RawFd = 0;

extern "C" fn signal_handler(_: i32) {
    write(unsafe { SIGNAL_FD }, &[0u8; 1]).unwrap();
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

    pub fn interact(&mut self) -> io::Result<()> {
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
        let mut buf = [0u8; 1024];

        // Set up a self-pipe for handling SIGWINCH
        let (signal_r, signal_w): (RawFd, RawFd) = pipe().unwrap();
        let signal_r = unsafe { OwnedFd::from_raw_fd(signal_r) };
        let signal_w = unsafe { OwnedFd::from_raw_fd(signal_w) };

        unsafe {
            SIGNAL_FD = signal_w.as_raw_fd();
        }

        fd_set.insert(&signal_r);

        let sig_action = SigAction::new(
            SigHandler::Handler(signal_handler),
            nix::sys::signal::SaFlags::SA_RESTART,
            SigSet::empty(),
        );

        // Register the action for SIGWINCH
        unsafe {
            sigaction(Signal::SIGWINCH, &sig_action).unwrap();
        }

        // Call select in a loop and handle incoming data
        while self.process.status() == Some(WaitStatus::StillAlive) {
            let mut select_timeout = TimeVal::new(4, 0);
            let mut select_set = fd_set.clone();

            let res = select::select(None, &mut select_set, None, None, &mut select_timeout);
            if res.is_err() {
                if res.unwrap_err() == Errno::EINTR {
                    println!("INTERUPTED");
                    continue;
                } else {
                    eprintln!("select error: {:?}", res);
                }
            } else {
                if select_set.contains(&process_stdout_fd) {
                    let bytes_read = self.process_stdout.read(&mut buf)?;
                    std::io::stdout().write_all(&buf[..bytes_read])?;
                    std::io::stdout().flush()?;
                }

                if select_set.contains(&stdin_fd) {
                    let bytes_read = std::io::stdin().read(&mut buf)?;
                    self.process_stdin.write_all(&buf[..bytes_read])?;
                    self.process_stdin.flush()?;
                }

                if select_set.contains(&signal_r) {
                    // drain the pipe
                    read(signal_r.as_raw_fd(), &mut buf)?;

                    // get window size
                    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
                    let res =
                        unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) };
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
