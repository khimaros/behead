//! android auto protocol, phone side. no io and no std, so the same core
//! runs on linux hosts and on microcontrollers.
#![no_std]

extern crate alloc;

pub mod accessory;
pub mod frame;
pub mod h264;
pub mod proto;
pub mod session;
pub mod text;

pub use session::{AudioFormat, Error, Event, Options, Session, Tls, VideoMode};
