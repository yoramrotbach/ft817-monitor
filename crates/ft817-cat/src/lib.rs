//! Shared FT-817/818 CAT code used by 817 Monitor and 817 EEPROM.
//!
//! This crate can only build read commands (see `cat::frame`). Code that
//! writes to the radio lives exclusively in the 817 EEPROM app.

pub mod cat;
pub mod sim;
pub mod transport;
