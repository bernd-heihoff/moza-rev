use std::env;
use std::error::Error;
use std::io;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use moza_rev::device::moza_led::MozaLedDevice;
use moza_rev::led::blue_shift::BlueShiftMapper;
use moza_rev::moza::{self, Moza, Protocol};
use moza_rev::telemetry::engine::EngineSample;
use moza_rev::telemetry::pc2::{self, DEFAULT_PORT as PC2_PORT};

const LED_COUNT: usize = 10;
const IDLE_TIMEOUT: Duration = Duration::from_secs(2);
const HEARTBEAT: Duration = Duration::from_millis(250);
const READ_TIMEOUT: Duration = Duration::from_millis(20);

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
    let leds = MozaLedDevice::new(LED_COUNT);

    // This replaces the need to click Boxflat's telemetry test.
    leds.initialize(&mut wheel, &normal_colors)?;

    let socket = UdpSocket::bind(("0.0.0.0", PC2_PORT))?;
    socket.set_read_timeout(Some(READ_TIMEOUT))?;
    println!("Listening for PC2/Madness telemetry on UDP {PC2_PORT}");

    let mut buffer = vec![0_u8; 2048];
    let mut latest: Option<(EngineSample, Instant)> = None;
    let mut mapper = BlueShiftMapper::new(
        LED_COUNT,
        led_start,
        flash_at,
        hysteresis,
        Duration::from_millis(flash_ms),
        Instant::now(),
    );
    let mut last_mask: Option<u32> = None;
    let mut last_send = Instant::now();
    let mut last_status = Instant::now();

    loop {
        match socket.recv(&mut buffer) {
            Ok(length) => {
                if let Some(sample) = pc2::decode(&buffer[..length]) {
                    latest = Some((sample, Instant::now()));
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
        let active = latest.as_ref().and_then(|(sample, received)| {
            if now.duration_since(*received) < IDLE_TIMEOUT {
                Some(sample)
            } else {
                None
            }
        });

        let active_ratio = active.and_then(EngineSample::rpm_ratio);
        let ratio = active_ratio.unwrap_or(0.0);
        let output = mapper.update(active_ratio, now);

        if output.flash_changed {
            // Turn everything off before changing the runtime color table.
            leds.set_mask(&mut wheel, 0)?;

            if output.flash_mode {
                leds.set_colors(&mut wheel, &flash_colors)?;
                println!("Blue shift flash active");
            } else {
                leds.set_colors(&mut wheel, &normal_colors)?;
                println!("Normal RPM colors restored");
            }

            last_mask = None;
        }

        let target_mask = output.mask;

        if last_mask != Some(target_mask) || last_send.elapsed() >= HEARTBEAT {
            leds.set_mask(&mut wheel, target_mask)?;
            last_mask = Some(target_mask);
            last_send = now;
        }

        if last_status.elapsed() >= Duration::from_secs(1) {
            if let Some(sample) = active {
                println!(
                    "rpm {:>5}/{:<5} {:>5.1}%  mask=0x{:03X}  {}",
                    sample.rpm,
                    sample.redline_rpm,
                    ratio * 100.0,
                    target_mask,
                    if output.flash_mode { "FLASH" } else { "normal" },
                );
            } else {
                println!("Waiting for PC2/Madness telemetry");
            }

            last_status = now;
        }
    }
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
