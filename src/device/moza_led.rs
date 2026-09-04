use std::io;

use crate::moza::{Moza, Protocol};

/// Semantic RPM-LED facade over the low-level MOZA serial client.
///
/// This keeps callers independent of the concrete MOZA commands used to
/// enable telemetry mode, upload runtime colors, and drive the LED mask.
pub struct MozaLedDevice {
    moza: Moza,
    led_count: usize,
}

impl MozaLedDevice {
    pub fn open(path: &str, protocol: Protocol, led_count: usize) -> io::Result<Self> {
        Ok(Self {
            moza: Moza::open(path, protocol)?,
            led_count,
        })
    }

    /// Enable host-driven RPM LEDs, install the initial color table, and
    /// start with all LEDs off.
    pub fn initialize(&mut self, colors: &[[u8; 3]]) -> io::Result<()> {
        self.moza.set_rpm_telemetry_mode()?;
        self.set_colors(colors)?;
        self.set_mask(0)
    }

    /// Upload the temporary runtime RPM color table.
    pub fn set_colors(&mut self, colors: &[[u8; 3]]) -> io::Result<()> {
        self.moza.send_telemetry_rpm_colors(colors)
    }

    /// Set the logical LED mask using this device's configured LED count.
    pub fn set_mask(&mut self, mask: u32) -> io::Result<()> {
        self.moza.send_rpm_bitmask(mask, self.led_count)
    }
}
