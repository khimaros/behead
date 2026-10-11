//! the microsd card, as files under a path: the chip's sd host on its own
//! pins, four data lines wide, with esp-idf's fat filesystem on top.

use esp_idf_svc::fs::fatfs::Fatfs;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::ldo::{LdoChannel, LdoChannelConfig};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::sd::mmc::{SdMmcHostConfiguration, SdMmcHostDriver};
use esp_idf_svc::hal::sd::{SdCardConfiguration, SdCardDriver};
use esp_idf_svc::io::vfs::MountedFatfs;
use esp_idf_svc::sys::EspError;

/// where the card's files are once it is mounted
pub const ROOT: &str = "/sdcard";
const DRIVE: u8 = 0;
/// files open at once: the track playing, and a folder being read
const MAX_OPEN: usize = 4;
/// the card's pins are fed by the chip's fourth regulator, which is off
/// until asked for
const SUPPLY_MV: i32 = 3300;

/// mount the card that is in the slot, for as long as the firmware runs. one
/// put in later is not seen
pub fn mount() -> Result<(), EspError> {
    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;
    let supply = LdoChannel::new(peripherals.ldo4, &LdoChannelConfig::new(SUPPLY_MV))?;
    // the pins of the chip's first sd slot, which no setting moves
    let host = SdMmcHostDriver::new_4bits(
        peripherals.sdmmc0,
        pins.gpio44,
        pins.gpio43,
        pins.gpio39,
        pins.gpio40,
        pins.gpio41,
        pins.gpio42,
        None::<AnyIOPin>,
        None::<AnyIOPin>,
        &SdMmcHostConfiguration::new(),
    )?;
    let card = SdCardDriver::new_mmc(host, &SdCardConfiguration::new())?;
    let mounted = MountedFatfs::mount(Fatfs::new_sdcard(DRIVE, card)?, ROOT, MAX_OPEN)?;
    // dropping either would take the card away again
    std::mem::forget((supply, mounted));
    Ok(())
}
