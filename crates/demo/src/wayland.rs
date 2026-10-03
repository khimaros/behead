//! a minimal wayland client: one fullscreen xdg toplevel drawn through
//! wl_shm, with touch and keyboard input. it speaks the wire protocol
//! directly and covers only the requests and events the demo needs.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;

const DISPLAY: u32 = 1;
const REGISTRY: u32 = 2;
const DEFAULT_DISPLAY: &str = "wayland-0";
/// used until the compositor asks for a size
const DEFAULT_SIZE: (u32, u32) = (800, 480);
const BUFFERS: usize = 2;
const BYTES_PER_PIXEL: u32 = 4;
const FORMAT_XRGB8888: u32 = 1;
const SEAT_KEYBOARD: u32 = 2;
const SEAT_TOUCH: u32 = 4;
const KEY_PRESSED: u32 = 1;
const APP_ID: &str = "behead-demo";
const READ_BUFFER: usize = 4096;
/// room for the file descriptors one message can carry
const MAX_FDS: usize = 28;
/// globals this client binds, with the highest version it speaks
const COMPOSITOR: (&str, u32) = ("wl_compositor", 4);
const SHM: (&str, u32) = ("wl_shm", 1);
const WM_BASE: (&str, u32) = ("xdg_wm_base", 1);
const SEAT: (&str, u32) = ("wl_seat", 5);

pub enum Input {
    /// touch id, x, y in surface pixels
    Down(u32, u32, u32),
    Motion(u32, u32, u32),
    Up(u32),
    Cancel,
    /// a linux key code, and whether it went down
    Key(u32, bool),
}

pub trait App {
    fn input(&mut self, input: Input);
    /// rgb24 pixels for the next frame, at this size
    fn draw(&mut self, width: u32, height: u32) -> &[u8];
}

#[derive(Clone, Copy)]
enum Arg<'a> {
    Uint(u32),
    Int(i32),
    Str(&'a str),
    Fd(RawFd),
}

/// event arguments, read in order
struct Args<'a>(&'a [u8]);

impl Args<'_> {
    fn take(&mut self, count: usize) -> &[u8] {
        let (head, rest) = self.0.split_at(count.min(self.0.len()));
        self.0 = rest;
        head
    }

    fn uint(&mut self) -> u32 {
        self.take(4).try_into().map_or(0, u32::from_ne_bytes)
    }

    /// a wl_fixed surface coordinate, in whole pixels, never negative
    fn coordinate(&mut self) -> u32 {
        (self.uint() as i32 >> 8).max(0) as u32
    }

    fn string(&mut self) -> String {
        let length = self.uint() as usize;
        let bytes = self.take(length.div_ceil(4) * 4);
        String::from_utf8_lossy(&bytes[..length.saturating_sub(1).min(bytes.len())]).into_owned()
    }
}

/// the socket with its outgoing queue, incoming buffer and object ids
struct Connection {
    socket: UnixStream,
    out: Vec<u8>,
    out_fds: Vec<RawFd>,
    input: Vec<u8>,
    fds: VecDeque<OwnedFd>,
    /// ids the compositor has released. they are reused first, because the
    /// compositor refuses a new id that skips past unused ones
    free_ids: Vec<u32>,
    next_id: u32,
}

impl Connection {
    fn open() -> Result<Self, String> {
        let name = std::env::var("WAYLAND_DISPLAY").unwrap_or(DEFAULT_DISPLAY.into());
        let path = match name.starts_with('/') {
            true => name.into(),
            false => {
                let runtime = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "wayland: XDG_RUNTIME_DIR is unset")?;
                std::path::Path::new(&runtime).join(name)
            }
        };
        let socket = UnixStream::connect(&path).map_err(|e| format!("wayland: {}: {e}", path.display()))?;
        let (out, out_fds, input, fds, free_ids) = Default::default();
        let mut connection = Self { socket, out, out_fds, input, fds, free_ids, next_id: REGISTRY + 1 };
        connection.request(DISPLAY, 1, &[Arg::Uint(REGISTRY)]);
        Ok(connection)
    }

    fn new_id(&mut self) -> u32 {
        self.free_ids.pop().unwrap_or_else(|| {
            self.next_id += 1;
            self.next_id - 1
        })
    }

    fn bind(&mut self, name: u32, (interface, ours): (&str, u32), offered: u32) -> u32 {
        let id = self.new_id();
        self.request(REGISTRY, 0, &[Arg::Uint(name), Arg::Str(interface), Arg::Uint(ours.min(offered)), Arg::Uint(id)]);
        id
    }

    fn request(&mut self, object: u32, opcode: u16, args: &[Arg]) {
        let mut body = Vec::new();
        for arg in args {
            match *arg {
                Arg::Uint(value) => body.extend(value.to_ne_bytes()),
                Arg::Int(value) => body.extend(value.to_ne_bytes()),
                Arg::Str(text) => {
                    body.extend((text.len() as u32 + 1).to_ne_bytes());
                    body.extend(text.bytes().chain([0]));
                    body.resize(body.len().div_ceil(4) * 4, 0);
                }
                Arg::Fd(fd) => self.out_fds.push(fd),
            }
        }
        self.out.extend(object.to_ne_bytes());
        self.out.extend((((8 + body.len() as u32) << 16) | opcode as u32).to_ne_bytes());
        self.out.extend(body);
    }

    /// send queued requests, with their file descriptors alongside
    fn flush(&mut self) -> Result<(), String> {
        if self.out.is_empty() {
            return Ok(());
        }
        let fd_bytes = size_of_val(self.out_fds.as_slice());
        // u64 keeps the control buffer aligned for cmsghdr
        let mut control = [0u64; MAX_FDS];
        let mut iov = libc::iovec { iov_base: self.out.as_mut_ptr().cast(), iov_len: self.out.len() };
        // SAFETY: msghdr is plain data, valid when zeroed
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        (message.msg_iov, message.msg_iovlen) = (&mut iov, 1);
        if fd_bytes > 0 {
            message.msg_control = control.as_mut_ptr().cast();
            // SAFETY: CMSG_SPACE only computes a size
            message.msg_controllen = unsafe { libc::CMSG_SPACE(fd_bytes as u32) } as _;
            // SAFETY: the control buffer holds one header plus fd_bytes, as just declared
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&message);
                ((*header).cmsg_level, (*header).cmsg_type) = (libc::SOL_SOCKET, libc::SCM_RIGHTS);
                (*header).cmsg_len = libc::CMSG_LEN(fd_bytes as u32) as _;
                std::ptr::copy_nonoverlapping(self.out_fds.as_ptr().cast(), libc::CMSG_DATA(header), fd_bytes);
            }
        }
        // SAFETY: every pointer in the message refers to a live local buffer
        let sent = unsafe { libc::sendmsg(self.socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
        if sent < 0 {
            return Err(format!("wayland: {}", std::io::Error::last_os_error()));
        }
        self.socket.write_all(&self.out[sent as usize..]).map_err(|e| format!("wayland: {e}"))?;
        self.out.clear();
        self.out_fds.clear();
        Ok(())
    }

    /// keep file descriptors that arrived with a read, in order
    ///
    /// SAFETY: message must have just been filled in by recvmsg
    unsafe fn collect_fds(&mut self, message: &libc::msghdr) {
        // SAFETY: the control messages are the ones recvmsg wrote, and each
        // SCM_RIGHTS payload holds descriptors this process now owns
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(message);
            while !header.is_null() {
                if ((*header).cmsg_level, (*header).cmsg_type) == (libc::SOL_SOCKET, libc::SCM_RIGHTS) {
                    let count = ((*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / size_of::<RawFd>();
                    let data = libc::CMSG_DATA(header).cast::<RawFd>();
                    self.fds.extend((0..count).map(|i| OwnedFd::from_raw_fd(data.add(i).read_unaligned())));
                }
                header = libc::CMSG_NXTHDR(message, header);
            }
        }
    }

    /// block until events arrive, and return each complete one as
    /// (object, opcode, arguments)
    fn read(&mut self) -> Result<Vec<(u32, u16, Vec<u8>)>, String> {
        let (mut buffer, mut control) = ([0u8; READ_BUFFER], [0u64; MAX_FDS]);
        let mut iov = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
        // SAFETY: msghdr is plain data, valid when zeroed
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        (message.msg_iov, message.msg_iovlen) = (&mut iov, 1);
        (message.msg_control, message.msg_controllen) = (control.as_mut_ptr().cast(), size_of_val(&control) as _);
        // SAFETY: every pointer in the message refers to a live local buffer
        let received = unsafe { libc::recvmsg(self.socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
        match received {
            0 => return Err("wayland: the compositor closed the connection".into()),
            n if n < 0 => return Err(format!("wayland: {}", std::io::Error::last_os_error())),
            n => self.input.extend_from_slice(&buffer[..n as usize]),
        }
        // SAFETY: recvmsg just filled in the message
        unsafe { self.collect_fds(&message) };
        let mut events = Vec::new();
        while self.input.len() >= 8 {
            let word = |at: usize| u32::from_ne_bytes(self.input[at..at + 4].try_into().unwrap());
            let (object, header) = (word(0), word(4));
            let size = (header >> 16) as usize;
            if size < 8 || self.input.len() < size {
                break;
            }
            events.push((object, header as u16, self.input[8..size].to_vec()));
            self.input.drain(..size);
        }
        Ok(events)
    }
}

/// shared memory holding the buffers, all at one size
struct Pool {
    id: u32,
    file: File,
    width: u32,
    height: u32,
    /// buffer ids, and whether the compositor still holds each
    buffers: [(u32, bool); BUFFERS],
}

impl Pool {
    /// a pool of buffers at this size, replacing any old one
    fn create(connection: &mut Connection, shm: u32, (width, height): (u32, u32)) -> Result<Self, String> {
        let (stride, frame_bytes) = (width * BYTES_PER_PIXEL, width * height * BYTES_PER_PIXEL);
        // SAFETY: the name is a nul terminated literal
        let fd = unsafe { libc::memfd_create(c"behead-demo".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(format!("memfd: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: memfd_create just returned this descriptor, owned by nothing else
        let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        let size = frame_bytes as usize * BUFFERS;
        file.set_len(size as u64).map_err(|e| format!("memfd: {e}"))?;
        let id = connection.new_id();
        connection.request(shm, 0, &[Arg::Uint(id), Arg::Fd(file.as_raw_fd()), Arg::Int(size as i32)]);
        let buffers = std::array::from_fn(|index| {
            let buffer = connection.new_id();
            let offset = index as u32 * frame_bytes;
            let geometry = [offset, width, height, stride, FORMAT_XRGB8888].map(|v| Arg::Int(v as i32));
            connection.request(id, 0, &[[Arg::Uint(buffer)].as_slice(), &geometry].concat());
            (buffer, false)
        });
        // send the fd while the pool still holds it open
        connection.flush()?;
        Ok(Self { id, file, width, height, buffers })
    }

    fn destroy(&self, connection: &mut Connection) {
        self.buffers.iter().for_each(|&(id, _)| connection.request(id, 0, &[]));
        connection.request(self.id, 1, &[]);
    }
}

/// ids of the objects this client creates. 0 means not yet created
#[derive(Default)]
struct Window {
    compositor: u32,
    shm: u32,
    wm_base: u32,
    seat: u32,
    surface: u32,
    xdg_surface: u32,
    toplevel: u32,
    touch: u32,
    keyboard: u32,
    frame: u32,
    size: Option<(u32, u32)>,
    configured: bool,
    frame_due: bool,
    closed: bool,
}

struct Client {
    connection: Connection,
    window: Window,
    pool: Option<Pool>,
}

impl Client {
    /// create the toplevel once the globals it needs are bound
    fn create_window(&mut self) {
        let Client { connection, window, .. } = self;
        if window.surface != 0 || window.compositor == 0 || window.wm_base == 0 || window.shm == 0 {
            return;
        }
        window.surface = connection.new_id();
        connection.request(window.compositor, 0, &[Arg::Uint(window.surface)]);
        window.xdg_surface = connection.new_id();
        connection.request(window.wm_base, 2, &[Arg::Uint(window.xdg_surface), Arg::Uint(window.surface)]);
        window.toplevel = connection.new_id();
        connection.request(window.xdg_surface, 1, &[Arg::Uint(window.toplevel)]);
        connection.request(window.toplevel, 3, &[Arg::Str(APP_ID)]);
        connection.request(window.toplevel, 11, &[Arg::Uint(0)]);
        connection.request(window.surface, 6, &[]);
    }

    /// draw into a buffer the compositor is not holding, and ask for the next frame
    fn draw(&mut self, app: &mut impl App) -> Result<(), String> {
        let Client { connection, window, pool } = self;
        let Some(pool) = pool.as_mut() else { return Ok(()) };
        let Some(index) = pool.buffers.iter().position(|&(_, busy)| !busy) else { return Ok(()) };
        let rgb = app.draw(pool.width, pool.height);
        let pixels: Vec<u8> = rgb.chunks_exact(3).flat_map(|p| [p[2], p[1], p[0], 0xff]).collect();
        pool.file.write_all_at(&pixels, index as u64 * pixels.len() as u64).map_err(|e| format!("memfd: {e}"))?;
        pool.buffers[index].1 = true;
        connection.request(window.surface, 1, &[Arg::Uint(pool.buffers[index].0), Arg::Int(0), Arg::Int(0)]);
        connection.request(window.surface, 9, &[0, 0, pool.width as i32, pool.height as i32].map(Arg::Int));
        window.frame = connection.new_id();
        connection.request(window.surface, 3, &[Arg::Uint(window.frame)]);
        connection.request(window.surface, 6, &[]);
        window.frame_due = false;
        Ok(())
    }

    fn dispatch(&mut self, app: &mut impl App, object: u32, opcode: u16, body: &[u8]) -> Result<(), String> {
        let Client { connection, window, pool } = self;
        let mut args = Args(body);
        match (object, opcode) {
            (DISPLAY, 0) => {
                let (object, code, message) = (args.uint(), args.uint(), args.string());
                return Err(format!("wayland: error {code} on object {object}: {message}"));
            }
            (DISPLAY, 1) => connection.free_ids.push(args.uint()),
            (REGISTRY, 0) => {
                let (name, interface, version) = (args.uint(), args.string(), args.uint());
                let (slot, global) = match interface.as_str() {
                    "wl_compositor" => (&mut window.compositor, COMPOSITOR),
                    "wl_shm" => (&mut window.shm, SHM),
                    "xdg_wm_base" => (&mut window.wm_base, WM_BASE),
                    "wl_seat" => (&mut window.seat, SEAT),
                    _ => return Ok(()),
                };
                if *slot == 0 {
                    *slot = connection.bind(name, global, version);
                }
            }
            (id, 0) if id == window.wm_base => connection.request(id, 3, &[Arg::Uint(args.uint())]),
            (id, 0) if id == window.seat => {
                let capabilities = args.uint();
                if capabilities & SEAT_KEYBOARD != 0 && window.keyboard == 0 {
                    window.keyboard = connection.new_id();
                    connection.request(id, 1, &[Arg::Uint(window.keyboard)]);
                }
                if capabilities & SEAT_TOUCH != 0 && window.touch == 0 {
                    window.touch = connection.new_id();
                    connection.request(id, 2, &[Arg::Uint(window.touch)]);
                }
            }
            (id, 0) if id == window.toplevel => {
                let size = (args.uint(), args.uint());
                window.size = Some(size).filter(|&(width, height)| width > 0 && height > 0).or(window.size);
            }
            (id, 1) if id == window.toplevel => window.closed = true,
            (id, 0) if id == window.xdg_surface => {
                connection.request(id, 4, &[Arg::Uint(args.uint())]);
                (window.configured, window.frame_due) = (true, true);
                let size = window.size.unwrap_or(DEFAULT_SIZE);
                if pool.as_ref().is_none_or(|pool| (pool.width, pool.height) != size) {
                    if let Some(old) = pool.take() {
                        old.destroy(connection);
                    }
                    *pool = Some(Pool::create(connection, window.shm, size)?);
                }
            }
            (id, 0) if id == window.frame => (window.frame, window.frame_due) = (0, true),
            (id, 0) if id == window.touch => {
                let (_serial, _time, _surface, touch) = (args.uint(), args.uint(), args.uint(), args.uint());
                app.input(Input::Down(touch, args.coordinate(), args.coordinate()));
            }
            (id, 1) if id == window.touch => {
                let (_serial, _time, touch) = (args.uint(), args.uint(), args.uint());
                app.input(Input::Up(touch));
            }
            (id, 2) if id == window.touch => {
                let (_time, touch) = (args.uint(), args.uint());
                app.input(Input::Motion(touch, args.coordinate(), args.coordinate()));
            }
            (id, 4) if id == window.touch => app.input(Input::Cancel),
            // the keymap: this client reports raw key codes, so close it
            (id, 0) if id == window.keyboard => drop(connection.fds.pop_front()),
            (id, 3) if id == window.keyboard => {
                let (_serial, _time, key, state) = (args.uint(), args.uint(), args.uint(), args.uint());
                app.input(Input::Key(key, state == KEY_PRESSED));
            }
            // wl_buffer.release, the only other opcode 0 event this client acts on
            (id, 0) => {
                if let Some(buffer) = pool.iter_mut().flat_map(|pool| &mut pool.buffers).find(|(b, _)| *b == id) {
                    buffer.1 = false;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// show the app full screen until the compositor closes the window
pub fn run(app: &mut impl App) -> Result<(), String> {
    let mut client = Client { connection: Connection::open()?, window: Window::default(), pool: None };
    while !client.window.closed {
        client.connection.flush()?;
        for (object, opcode, body) in client.connection.read()? {
            client.dispatch(app, object, opcode, &body)?;
        }
        client.create_window();
        if client.window.configured && client.window.frame_due {
            client.draw(app)?;
        }
    }
    Ok(())
}
