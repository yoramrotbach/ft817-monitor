//! Snapshot of what the radio is doing, sent to the UI as JSON.
//! `None` means "not known from this connection type" and renders as a dash.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Link {
    #[default]
    Stopped,
    Connecting,
    Up,
    Lost,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RigState {
    pub link: Link,
    /// Short status: "Connecting (attempt 2)…", "Next attempt in 4 s", "Connected".
    pub message: String,
    /// The last connection problem. Stays set through retries (so the user can
    /// read it) and is cleared only once the link is up again.
    pub error: Option<String>,
    /// Connection attempt number since the link was last up (0 while up).
    pub attempt: u32,
    /// Seconds until the next connection attempt while waiting to retry.
    pub retry_in: Option<u32>,
    pub source: String, // "Serial COM3 @ 38400", "Demo"
    pub polls_per_sec: f32,
    pub errors: u32,

    pub freq_hz: Option<u64>,
    pub mode: Option<String>,
    pub band: Option<String>,
    pub vfo: Option<String>,

    pub tx: bool,
    // receive
    pub s_raw: Option<u8>,
    pub s_label: Option<String>,
    pub squelched: Option<bool>,
    pub tone_mismatch: Option<bool>,
    pub discr_off: Option<bool>,
    // transmit (0..15 raw scale)
    pub po: Option<u8>,
    pub swr: Option<u8>,
    pub alc: Option<u8>,
    pub modulation: Option<u8>,
    pub high_swr: Option<bool>,

    // panel settings (EEPROM)
    pub agc: Option<String>,
    pub nb: Option<bool>,
    pub dsp: Option<bool>,
    pub pbt: Option<bool>,
    pub lock: Option<bool>,
    pub fst: Option<bool>,
    pub vox: Option<bool>,
    pub bk: Option<bool>,
    pub kyr: Option<bool>,
    pub split: Option<bool>,
    pub tx_power: Option<String>,
    pub meter_mode: Option<String>,
    pub antenna: Option<String>,
    pub lcd_color: Option<String>,
}
