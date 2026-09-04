use std::env;
use std::error::Error;
use std::io;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use moza_rev::madness::TelemetryPacket;
use moza_rev::moza::{self, Moza, Protocol};

const LED_COUNT: usize = 10;
const AMS2_PORT: u16 = 5606;
const IDLE_TIMEOUT: Duration = Duration::from_secs(2);
const HEARTBEAT: Duration = Duration::from_millis(250);

const DEFAULT_NORMAL_COLORS: [[u8; 3]; LED_COUNT] = [
    [0, 255, 0],
    [0, 255, 0],
    [0, 255, 0],
    [0, 255, 0],
    [255, 255, 0],
    [255, 255, 0],
    [255, 255, 0],
    [255, 128, 0],
    [255, 0, 0],
    [255, 0, 0],
];

struct EngineSample {
    rpm: i32,
    redline: i32,
    received: Instant,
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let led_start = env_fraction("MOZA_LED_START", 0.70);
    let flash_at = env_fraction("MOZA_FLASH_AT", 0.95).clamp(0.05, 1.0);
    let led_start = led_start.clamp(0.0, flash_at - 0.01);
    let hysteresis = env_fraction("MOZA_FLASH_HYSTERESIS", 0.02).clamp(0.0, flash_at);
    let flash_ms = env_u64("MOZA_FLASH_MS", 100).max(40);

    let normal_colors = load_normal_colors();
    let flash_color = env::var("MOZA_FLASH_COLOR")
        .ok()
        .as_deref()
        .and_then(parse_rgb)
        .unwrap_or([0, 0, 255]);
    let flash_colors = vec![flash_color; LED_COUNT];

    let serial_path = moza::find_wheelbase()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no MOZA wheelbase found"))?;

    let protocol = moza::detect_protocol(&serial_path);
    if protocol != Protocol::Modern {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this shift-light binary requires the modern MOZA protocol",
        )
        .into());
    }

    println!("Opening {serial_path} ({protocol:?})");
    println!(
        "Mapping: first LED {:.0}%, blue flash {:.0}%, release {:.0}%, {} ms on/off",
        led_start * 100.0,
        flash_at * 100.0,
        (flash_at - hysteresis) * 100.0,
        flash_ms,
    );

    let mut wheel = Moza::open(&serial_path, protocol)?;

    // This replaces the need to click Boxflat's telemetry test.
    wheel.set_rpm_telemetry_mode()?;
    wheel.send_telemetry_rpm_colors(&normal_colors)?;
    wheel.send_rpm_bitmask(0, LED_COUNT)?;

    let socket = UdpSocket::bind(("0.0.0.0", AMS2_PORT))?;
    socket.set_read_timeout(Some(Duration::from_millis(20)))?;
    println!("Listening for AMS2 telemetry on UDP {AMS2_PORT}");

    let mut buffer = vec![0_u8; 2048];
    let mut latest: Option<EngineSample> = None;
    let mut flash_mode = false;
    let mut flash_started = Instant::now();
    let mut last_mask: Option<u32> = None;
    let mut last_send = Instant::now();
    let mut last_status = Instant::now();

    loop {
        match socket.recv(&mut buffer) {
            Ok(length) => {
                if let Some(packet) = TelemetryPacket::from_bytes(&buffer[..length]) {
                    let redline = packet.data.redline_rpm();

                    if redline > 0 {
                        latest = Some(EngineSample {
                            rpm: packet.data.rpm(),
                            redline,
                            received: Instant::now(),
                        });
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }

        let now = Instant::now();
        let active = latest
            .as_ref()
            .filter(|sample| now.duration_since(sample.received) < IDLE_TIMEOUT);

        let ratio = active
            .map(|sample| sample.rpm.max(0) as f32 / sample.redline as f32)
            .unwrap_or(0.0);

        // Hysteresis: enter at 95%, leave below 93% by default.
        let should_flash = if flash_mode {
            active.is_some() && ratio >= flash_at - hysteresis
        } else {
            active.is_some() && ratio >= flash_at
        };

        if should_flash != flash_mode {
            // Turn everything off before changing the runtime color table.
            wheel.send_rpm_bitmask(0, LED_COUNT)?;

            if should_flash {
                wheel.send_telemetry_rpm_colors(&flash_colors)?;
                flash_started = now;
                println!("Blue shift flash active");
            } else {
                wheel.send_telemetry_rpm_colors(&normal_colors)?;
                println!("Normal RPM colors restored");
            }

            flash_mode = should_flash;
            last_mask = None;
        }

        let target_mask = if active.is_none() {
            0
        } else if flash_mode {
            let phase = (now.duration_since(flash_started).as_millis() / flash_ms as u128) % 2;

            if phase == 0 { full_mask() } else { 0 }
        } else {
            progressive_mask(ratio, led_start, flash_at)
        };

        if last_mask != Some(target_mask) || last_send.elapsed() >= HEARTBEAT {
            wheel.send_rpm_bitmask(target_mask, LED_COUNT)?;
            last_mask = Some(target_mask);
            last_send = now;
        }

        if last_status.elapsed() >= Duration::from_secs(1) {
            if let Some(sample) = active {
                println!(
                    "rpm {:>5}/{:<5} {:>5.1}%  mask=0x{:03X}  {}",
                    sample.rpm,
                    sample.redline,
                    ratio * 100.0,
                    target_mask,
                    if flash_mode { "FLASH" } else { "normal" },
                );
            } else {
                println!("Waiting for AMS2 telemetry");
            }

            last_status = now;
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

fn env_fraction(name: &str, default: f32) -> f32 {
    let mut value = env::var(name)
        .ok()
        .and_then(|text| text.parse::<f32>().ok())
        .unwrap_or(default);

    // Accept either 0.95 or 95.
    if value > 1.0 {
        value /= 100.0;
    }

    value.clamp(0.0, 1.0)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(default)
}

fn parse_rgb(text: &str) -> Option<[u8; 3]> {
    let text = text.trim().trim_start_matches('#');

    if text.len() != 6 || !text.is_ascii() {
        return None;
    }

    Some([
        u8::from_str_radix(&text[0..2], 16).ok()?,
        u8::from_str_radix(&text[2..4], 16).ok()?,
        u8::from_str_radix(&text[4..6], 16).ok()?,
    ])
}

fn load_normal_colors() -> Vec<[u8; 3]> {
    if let Ok(value) = env::var("MOZA_NORMAL_COLORS") {
        let entries: Vec<_> = value.split(',').collect();
        let parsed: Option<Vec<_>> = entries.iter().map(|entry| parse_rgb(entry)).collect();

        if entries.len() == LED_COUNT
            && let Some(colors) = parsed
        {
            return colors;
        }

        eprintln!("Invalid MOZA_NORMAL_COLORS; expected ten comma-separated RRGGBB values");
    }

    DEFAULT_NORMAL_COLORS.to_vec()
}
