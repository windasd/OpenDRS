//! OpenDRS firmware tools for the Kydera DR-8500UV / HORA DH-98VU: the IAP firmware upgrade,
//! the call-recording download, the pre-flight checks and the record-read patch for firmware
//! images, and the list of supported radios.
//!
//! A Rust port of the Python modules `iap.py`, `iap_sim.py`, `recording.py`,
//! `recording_sim.py`, `firmware_check.py`, `fwpatch.py` and `hardware.py` (src/opendrs), kept
//! byte-for-byte compatible with them: the same bytes on the wire and the same messages.

pub mod firmware_check;
pub mod fwpatch;
pub mod hardware;
pub mod iap;
pub mod iap_sim;
mod pytext;
pub mod recording;
pub mod recording_sim;

pub use pytext::sha256_hex;

/// Asked between steps of a long transfer: `true` stops it (the GUI's Stop button).
pub type Cancelled<'a> = Box<dyn FnMut() -> bool + 'a>;
