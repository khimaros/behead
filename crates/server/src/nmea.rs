//! the car's location as nmea 0183 sentences on a unix socket, the way a gps
//! receiver is shared with location services such as geoclue (its
//! nmea-socket option). a reader gets the latest fix when it connects, and
//! every fix after that.

use crate::sensors::{DEGREES_E6, E3, METRES_E2};
use aap::proto;
use std::fs::Permissions;
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEGREE_E7: i64 = 10_000_000;
/// minutes are written to five places
const MINUTE_E5: i64 = 100_000;
const SECONDS_PER_DAY: u64 = 86_400;
/// one knot in metres per second
const KNOT: f64 = 1852.0 / 3600.0;
/// metres of error per unit of horizontal dilution, the usual rule of thumb
const METRES_PER_HDOP: f64 = 5.0;
/// anyone may connect: the socket's directory decides who can reach it
const SOCKET_MODE: u32 = 0o666;

#[derive(Default)]
struct Feed {
    latest: Option<String>,
    readers: Vec<UnixStream>,
}

static FEED: OnceLock<Mutex<Feed>> = OnceLock::new();

/// degrees and decimal minutes with the hemisphere, from degrees times 1e7
fn coordinate(value: i32, degree_digits: usize, hemispheres: [char; 2]) -> String {
    let (size, hemisphere) = (i64::from(value).abs(), hemispheres[usize::from(value < 0)]);
    let minutes = size % DEGREE_E7 * 60 * MINUTE_E5 / DEGREE_E7;
    format!("{:0degree_digits$}{:02}.{:05},{hemisphere}", size / DEGREE_E7, minutes / MINUTE_E5, minutes % MINUTE_E5)
}

/// a scaled value to some decimal places, or an empty field without one
fn number(value: Option<impl Into<f64>>, scale: f64, places: usize) -> String {
    value.map_or(String::new(), |value| format!("{:.places$}", value.into() / scale))
}

fn sentence(body: String) -> String {
    let checksum = body.bytes().fold(0, |sum, byte| sum ^ byte);
    format!("${body}*{checksum:02X}\r\n")
}

/// a fix as a gga and an rmc sentence, which between them carry all of it,
/// or None for a fix without a position. the time of day is the one given,
/// since readers compare it with their own clock rather than the car's
pub fn sentences(fix: &proto::Location, since_epoch: Duration) -> Option<String> {
    let (latitude, longitude) = (coordinate(fix.latitude?, 2, ['N', 'S']), coordinate(fix.longitude?, 3, ['E', 'W']));
    let position = format!("{latitude},{longitude}");
    let seconds = since_epoch.as_secs() % SECONDS_PER_DAY;
    let hundredths = since_epoch.subsec_millis() / 10;
    let clock = format!("{:02}{:02}{:02}.{hundredths:02}", seconds / 3600, seconds / 60 % 60, seconds % 60);
    let dilution = number(fix.accuracy, E3 * METRES_PER_HDOP, 1);
    let altitude = number(fix.altitude, METRES_E2, 1);
    let (knots, course) = (number(fix.speed, E3 * KNOT, 2), number(fix.bearing, DEGREES_E6, 1));
    let fix_data = sentence(format!("GPGGA,{clock},{position},1,,{dilution},{altitude},M,,M,,"));
    Some(fix_data + &sentence(format!("GPRMC,{clock},A,{position},{knots},{course},,,,A")))
}

/// offer the car's location on a socket at this path, replacing a socket an
/// earlier run left there
pub fn listen(path: &Path) -> Result<(), String> {
    if path.symlink_metadata().is_ok_and(|found| found.file_type().is_socket()) {
        std::fs::remove_file(path).map_err(|e| format!("removing {}: {e}", path.display()))?;
    }
    let directory = path.parent().unwrap_or(path);
    std::fs::create_dir_all(directory).map_err(|e| format!("creating {}: {e}", directory.display()))?;
    let listener = UnixListener::bind(path).map_err(|e| format!("listen {}: {e}", path.display()))?;
    let open = Permissions::from_mode(SOCKET_MODE);
    std::fs::set_permissions(path, open).map_err(|e| format!("{}: {e}", path.display()))?;
    let feed = FEED.get_or_init(Mutex::default);
    std::thread::spawn(move || {
        for mut reader in listener.incoming().flatten() {
            // a reader that stops reading is dropped rather than waited for
            let mut feed = feed.lock().unwrap();
            let latest = feed.latest.as_deref().unwrap_or_default();
            if reader.set_nonblocking(true).is_ok() && reader.write_all(latest.as_bytes()).is_ok() {
                feed.readers.push(reader);
            }
        }
    });
    Ok(())
}

/// send a fix to every reader, if listen() was called
pub fn publish(fix: &proto::Location) {
    let Some(feed) = FEED.get() else { return };
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let Some(text) = sentences(fix, since_epoch) else { return };
    let mut feed = feed.lock().unwrap();
    feed.readers.retain_mut(|reader| reader.write_all(text.as_bytes()).is_ok());
    feed.latest = Some(text);
}
