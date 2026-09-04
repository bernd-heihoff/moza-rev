use std::time::{Duration, Instant};

pub const LED_COUNT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedOutput {
    pub mask: u32,
    pub flash_mode: bool,
    pub flash_changed: bool,
}

#[derive(Debug)]
pub struct BlueShiftMapper {
    led_start: f32,
    flash_at: f32,
    hysteresis: f32,
    flash_period: Duration,
    flash_mode: bool,
    flash_started: Instant,
}

impl BlueShiftMapper {
    pub fn new(
        led_start: f32,
        flash_at: f32,
        hysteresis: f32,
        flash_period: Duration,
        now: Instant,
    ) -> Self {
        Self {
            led_start,
            flash_at,
            hysteresis,
            flash_period,
            flash_mode: false,
            flash_started: now,
        }
    }

    pub fn update(&mut self, ratio: Option<f32>, now: Instant) -> LedOutput {
        let active = ratio.is_some();
        let ratio = ratio.unwrap_or(0.0);

        let should_flash = if self.flash_mode {
            active && ratio >= self.flash_at - self.hysteresis
        } else {
            active && ratio >= self.flash_at
        };

        let flash_changed = should_flash != self.flash_mode;

        if flash_changed && should_flash {
            self.flash_started = now;
        }

        self.flash_mode = should_flash;

        let mask = if !active {
            0
        } else if self.flash_mode {
            let period_ms = self.flash_period.as_millis().max(1);
            let phase = (now.duration_since(self.flash_started).as_millis() / period_ms) % 2;

            if phase == 0 { full_mask() } else { 0 }
        } else {
            progressive_mask(ratio, self.led_start, self.flash_at)
        };

        LedOutput {
            mask,
            flash_mode: self.flash_mode,
            flash_changed,
        }
    }
}

fn progressive_mask(ratio: f32, start: f32, full: f32) -> u32 {
    if ratio < start {
        return 0;
    }

    let fraction = ((ratio - start) / (full - start)).clamp(0.0, 1.0);
    // Divide the progressive range into ten equal RPM intervals.
    // The tenth LED therefore occupies the final full interval below flashing.
    let lit = 1 + (fraction * LED_COUNT as f32).floor() as usize;
    (1_u32 << lit.min(LED_COUNT)) - 1
}

fn full_mask() -> u32 {
    (1_u32 << LED_COUNT) - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: f32 = 0.70;
    const FLASH_AT: f32 = 0.95;
    const HYSTERESIS: f32 = 0.02;
    const FLASH_PERIOD: Duration = Duration::from_millis(100);

    fn mapper(now: Instant) -> BlueShiftMapper {
        BlueShiftMapper::new(START, FLASH_AT, HYSTERESIS, FLASH_PERIOD, now)
    }

    #[test]
    fn progressive_mapping_starts_at_first_led_and_reaches_all_leds() {
        assert_eq!(progressive_mask(START - 0.001, START, FLASH_AT), 0);
        assert_eq!(progressive_mask(START, START, FLASH_AT), 0b1);
        assert_eq!(progressive_mask(0.825, START, FLASH_AT), 0b11_1111);
        assert_eq!(progressive_mask(FLASH_AT, START, FLASH_AT), full_mask());
    }

    #[test]
    fn flash_mode_uses_hysteresis() {
        let now = Instant::now();
        let mut mapper = mapper(now);

        let entered = mapper.update(Some(FLASH_AT), now);
        assert!(entered.flash_mode);
        assert!(entered.flash_changed);

        let held = mapper.update(Some(0.94), now + Duration::from_millis(10));
        assert!(held.flash_mode);
        assert!(!held.flash_changed);

        let released = mapper.update(Some(0.929), now + Duration::from_millis(20));
        assert!(!released.flash_mode);
        assert!(released.flash_changed);
    }

    #[test]
    fn flash_mode_toggles_all_leds_on_and_off() {
        let now = Instant::now();
        let mut mapper = mapper(now);

        assert_eq!(mapper.update(Some(FLASH_AT), now).mask, full_mask());
        assert_eq!(
            mapper
                .update(Some(FLASH_AT), now + Duration::from_millis(99))
                .mask,
            full_mask()
        );
        assert_eq!(
            mapper
                .update(Some(FLASH_AT), now + Duration::from_millis(100))
                .mask,
            0
        );
        assert_eq!(
            mapper
                .update(Some(FLASH_AT), now + Duration::from_millis(200))
                .mask,
            full_mask()
        );
    }

    #[test]
    fn inactive_input_turns_leds_off_and_leaves_flash_mode() {
        let now = Instant::now();
        let mut mapper = mapper(now);

        mapper.update(Some(FLASH_AT), now);
        let idle = mapper.update(None, now + Duration::from_millis(10));

        assert_eq!(idle.mask, 0);
        assert!(!idle.flash_mode);
        assert!(idle.flash_changed);
    }
}
