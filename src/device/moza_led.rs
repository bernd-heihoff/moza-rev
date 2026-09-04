use std::io;

use crate::moza::Moza;

/// Semantic RPM-LED facade over the low-level MOZA serial client.
///
/// The facade does not own the serial client. This lets the main runtime keep
/// one `Moza` handle for LED writes, temperature reads, and reconnect logic.
pub struct MozaLedDevice {
    led_count: usize,
}

impl MozaLedDevice {
    pub fn new(led_count: usize) -> Self {
        Self { led_count }
    }

    /// Enable host-driven RPM LEDs, install the initial color table, and
    /// start with all LEDs off.
    pub fn initialize(&self, moza: &mut Moza, colors: &[[u8; 3]]) -> io::Result<()> {
        moza.set_rpm_telemetry_mode()?;
        self.set_colors(moza, colors)?;
        self.set_mask(moza, 0)
    }

    /// Upload the temporary runtime RPM color table.
    pub fn set_colors(&self, moza: &mut Moza, colors: &[[u8; 3]]) -> io::Result<()> {
        moza.send_telemetry_rpm_colors(colors)
    }

    /// Set the logical LED mask using this device's configured LED count.
    pub fn set_mask(&self, moza: &mut Moza, mask: u32) -> io::Result<()> {
        moza.send_rpm_bitmask(mask, self.led_count)
    }
}
