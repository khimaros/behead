//! the shell commands that produce video and audio and take microphone input

use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// how long a stopped command may take to exit before it is killed
const STOP_GRACE: Duration = Duration::from_secs(2);
const STOP_POLL: Duration = Duration::from_millis(20);

/// the template with every {name} replaced by its value
pub fn fill(template: &str, values: &[(&str, u32)]) -> String {
    let mut command = template.to_string();
    for (name, value) in values {
        command = command.replace(&format!("{{{name}}}"), &value.to_string());
    }
    command
}

/// start a command with piped stdin, in a process group of its own, so
/// stopping it reaches everything it started, such as a compositor session.
/// if the spawning thread dies first, the kernel sends the shell SIGTERM.
pub fn spawn(command: &str, stdout: Stdio) -> std::io::Result<Child> {
    let mut shell = Command::new("sh");
    shell.args(["-c", command]).stdin(Stdio::piped()).stdout(stdout).process_group(0);
    // SAFETY: the hook only calls prctl, which is async-signal-safe
    unsafe { shell.pre_exec(terminate_with_parent) };
    shell.spawn()
}

/// in a forked child: ask the kernel for SIGTERM when the spawning thread exits
fn terminate_with_parent() -> std::io::Result<()> {
    // SAFETY: PR_SET_PDEATHSIG takes a signal number and changes nothing else
    match unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error()),
    }
}

/// writes to this pipe fail instead of blocking when it is full, so a
/// command that does not keep up loses data rather than stalling the session
pub fn never_block(pipe: &impl AsRawFd) {
    // SAFETY: fcntl on a pipe descriptor this process owns
    unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
}

fn wait_briefly(child: &mut Child) {
    let deadline = Instant::now() + STOP_GRACE;
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        std::thread::sleep(STOP_POLL);
    }
}

/// whether the command's output has something to read within the timeout
pub fn readable_within(pipe: &impl AsRawFd, timeout: Duration) -> bool {
    let mut poll = libc::pollfd { fd: pipe.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: one pollfd, which outlives the call
    unsafe { libc::poll(&mut poll, 1, timeout.as_millis() as libc::c_int) != 0 }
}

/// ask a command's process group to stop, then make it
pub fn stop(child: &mut Child) {
    let group = child.id() as libc::pid_t;
    // SAFETY: signals the process group spawn created
    unsafe { libc::killpg(group, libc::SIGTERM) };
    wait_briefly(child);
    // SAFETY: as above; a group that already exited is harmless to signal
    unsafe { libc::killpg(group, libc::SIGKILL) };
    let _ = child.wait();
}

/// let a command whose input has closed finish what it was given, then stop
/// whatever is left of its group
pub fn finish(child: &mut Child) {
    wait_briefly(child);
    stop(child);
}
