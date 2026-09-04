// Runtime: spawn the per-game listeners (each in its own
// `crate::listeners::<game>` module), route their `EngineState`
// updates to a single channel, and drive the Moza wheel's LED bar +
// status output from there. Knows nothing about clap; `run` takes a
// fully-parsed `ListenArgs` from `crate::cli`, and nothing about
// game protocols; that lives in the per-listener modules.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};

use moza_rev::listeners::{self, EngineState, GameId, Update};
use moza_rev::moza::{self, BaseTemps, Moza, Protocol};

use crate::cli::ListenArgs;

const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(250);
const STATUS_INTERVAL: Duration = Duration::from_millis(500);

/// How often to retry opening the wheelbase after a disconnect. Linux
/// renumbers `/dev/ttyACMx` on replug, so each retry re-runs the
/// autodetection (unless the user pinned `--serial`).
const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);

/// Time without a packet from the active game before we revert to "no game"
/// and clear the LED bar.
const IDLE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often to read the wheelbase temperature sensors.
const TEMP_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Per-sensor read timeout. Each `read_base_temps` call does three sequential
/// reads, so worst-case wall time is 3x this if all sensors fail to respond.
const TEMP_READ_DEADLINE: Duration = Duration::from_millis(300);
/// `recv_timeout` window so the loop wakes regularly to do thermal polling
/// and idle-detection even when no telemetry is arriving.
const RECV_TIMEOUT: Duration = Duration::from_millis(250);

// Approximate "elevated" thresholds. Moza firmware auto-protects somewhere
// above these; the goal is to surface unusual heat well before that fires.
const MCU_TEMP_WARN_C: f32 = 75.0;
const MOSFET_TEMP_WARN_C: f32 = 70.0;
const MOTOR_TEMP_WARN_C: f32 = 95.0;

/// Open the Moza wheelbase, honouring an explicit `--serial` if the user
/// supplied one or autodetecting otherwise. Returns `None` if no wheel is
/// reachable; the caller chooses whether that's fatal (startup) or just a
/// reconnect-attempt failure (mid-run after a USB unplug).
fn open_wheel(
    user_serial: Option<&str>,
    user_protocol: Option<Protocol>,
) -> Option<(String, Protocol, Moza)> {
    let path = match user_serial {
        Some(p) => p.to_string(),
        None => moza::find_wheelbase()?,
    };
    let protocol = user_protocol.unwrap_or_else(|| moza::detect_protocol(&path));
    match Moza::open(&path, protocol) {
        Ok(w) => Some((path, protocol, w)),
        Err(e) => {
            debug!("open Moza at {path}: {e}");
            None
        }
    }
}

pub fn run(args: ListenArgs) -> ExitCode {
    let user_serial = args.serial.clone();
    let user_protocol = args.protocol.map(Protocol::from);

    // Try to open the wheel once at startup, but don't make it fatal: a
    // systemd service may launch before the wheel is plugged in. The
    // main loop's reconnect cycle picks it up whenever it appears.
    let (mut wheel, mut last_reconnect) = match open_wheel(user_serial.as_deref(), user_protocol) {
        Some((path, protocol, w)) => {
            info!("Moza wheelbase connected at {path} ({protocol:?} protocol)");
            (Some(w), Instant::now())
        }
        None => {
            warn!(
                "no Moza wheelbase found yet; will keep retrying every {}s. \
                 plug it in, or pass --serial /dev/ttyACMx to pin a path",
                RECONNECT_INTERVAL.as_secs()
            );
            // Make the first reconnect attempt fire on the very next loop
            // tick rather than waiting a full interval.
            (
                None,
                Instant::now()
                    .checked_sub(RECONNECT_INTERVAL)
                    .unwrap_or_else(Instant::now),
            )
        }
    };

    let (tx, rx) = mpsc::channel::<Update>();
    if !listeners::wreckfest_2::spawn(args.wf2_port, tx.clone()) {
        return ExitCode::FAILURE;
    }
    if !listeners::codemasters_legacy::spawn(args.dr2_port, tx.clone()) {
        return ExitCode::FAILURE;
    }
    if !listeners::madness::spawn(args.ams2_port, tx.clone()) {
        return ExitCode::FAILURE;
    }
    if !listeners::outgauge::spawn(args.beamng_port, tx.clone()) {
        return ExitCode::FAILURE;
    }
    if !listeners::forza::spawn(args.forza_port, tx.clone()) {
        return ExitCode::FAILURE;
    }
    listeners::assetto_corsa::spawn(args.ac_port, tx);

    let led_count = args.leds;
    let verbose = args.verbose;

    // When running alongside Boxflat, leave all serial reads to Boxflat.
    let temp_poll_enabled = std::env::var_os("MOZA_REV_NO_TEMP_POLL").is_none();

    // Only do the in-place `\r` rewrite when stdout is a real TTY. Piped
    // output (`moza-rev | tee log.txt`, systemd, journald, etc.) gets
    // newline-terminated lines instead.
    let inplace_status = !verbose && std::io::stdout().is_terminal();

    let mut last_bitmask: Option<u32> = None;
    let mut last_send = Instant::now();
    let mut last_status = Instant::now();
    let mut last_temp_poll = Instant::now() - TEMP_POLL_INTERVAL; // poll once on startup
    let mut active_game: Option<GameId> = None;
    let mut last_packet_at: Option<Instant> = None;
    let mut packets_since_status: u32 = 0;

    loop {
        // Reconnect attempt if the wheel got unplugged. Quiet at debug
        // until the open succeeds; info-level chatter every 3s would
        // be noisy. The transition log is in `open_wheel`'s caller.
        if wheel.is_none() && last_reconnect.elapsed() >= RECONNECT_INTERVAL {
            if let Some((path, _, w)) = open_wheel(user_serial.as_deref(), user_protocol) {
                info!("Moza wheelbase connected at {path}");
                wheel = Some(w);
                // Force the next heartbeat to re-send the bitmask so the
                // bar matches engine state immediately rather than waiting
                // for the next change.
                last_bitmask = None;
            }
            last_reconnect = Instant::now();
        }

        if temp_poll_enabled && last_temp_poll.elapsed() >= TEMP_POLL_INTERVAL {
            if let Some(w) = wheel.as_mut() {
                poll_temps(w);
            }
            last_temp_poll = Instant::now();
        }

        // Idle timeout: clear the active-game state so the next heartbeat
        // writes 0 to the bar.
        if let Some(t) = last_packet_at
            && t.elapsed() >= IDLE_TIMEOUT
            && let Some(game) = active_game
        {
            info!(
                "telemetry stream from {} stopped (no packets for {:?})",
                game.name(),
                IDLE_TIMEOUT
            );
            active_game = None;
        }

        // Heartbeat keepalive. Always write at least once per
        // HEARTBEAT_INTERVAL, even with no telemetry: this keeps the
        // bar refreshed AND surfaces a USB unplug as a write error
        // within ~1-2s, which would otherwise go unnoticed in idle mode.
        if last_send.elapsed() >= HEARTBEAT_INTERVAL {
            let bitmask = if active_game.is_some() {
                last_bitmask.unwrap_or(0)
            } else {
                0
            };
            wheel = try_write(wheel.take(), bitmask, led_count, &mut last_reconnect);
            last_bitmask = Some(bitmask);
            last_send = Instant::now();
        }

        let update = match rx.recv_timeout(RECV_TIMEOUT) {
            Ok(u) => u,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                error!("all listener threads exited; shutting down");
                return ExitCode::FAILURE;
            }
        };

        if active_game != Some(update.game) {
            info!("telemetry stream from {} started", update.game.name());
            active_game = Some(update.game);
        }
        last_packet_at = Some(Instant::now());
        packets_since_status += 1;

        let bitmask = rpm_to_bitmask(&update.engine, led_count);
        if Some(bitmask) != last_bitmask {
            wheel = try_write(wheel.take(), bitmask, led_count, &mut last_reconnect);
            last_bitmask = Some(bitmask);
            last_send = Instant::now();
        }

        if verbose {
            print_status(
                update.game,
                &update.engine,
                bitmask,
                led_count,
                packets_since_status,
                false, // verbose: one line per packet, never overwrite
            );
            packets_since_status = 0;
        } else if inplace_status && last_status.elapsed() >= STATUS_INTERVAL {
            // Rolling status only when stdout is a real TTY (so the `\r`
            // overwrite keeps it to one visible line). On systemd /
            // piped output the connect/disconnect lifecycle lines plus
            // `--verbose` are the way to see what's happening; flooding
            // the journal with 2 status lines per second is useless.
            print_status(
                update.game,
                &update.engine,
                bitmask,
                led_count,
                packets_since_status,
                inplace_status,
            );
            last_status = Instant::now();
            packets_since_status = 0;
        }
    }
}

/// Send a bitmask to the wheel, dropping the handle on failure so the
/// next loop tick triggers a reconnect attempt. Returns the handle on
/// success, `None` on failure or if `wheel` was already `None`.
fn try_write(
    wheel: Option<Moza>,
    bitmask: u32,
    led_count: usize,
    last_reconnect: &mut Instant,
) -> Option<Moza> {
    let mut w = wheel?;
    match w.send_rpm_bitmask(bitmask, led_count) {
        Ok(()) => Some(w),
        Err(e) => {
            warn!("Moza wheelbase write failed ({e}); will retry to connect");
            *last_reconnect = Instant::now();
            None
        }
    }
}

fn poll_temps(wheel: &mut Moza) {
    match wheel.read_base_temps(TEMP_READ_DEADLINE) {
        Ok(t) => log_temps(&t),
        Err(e) => debug!("temp read failed: {e}"),
    }
}

fn log_temps(t: &BaseTemps) {
    debug!(
        "temps  MCU={} MOSFET={} motor={}",
        fmt_temp(t.mcu_c),
        fmt_temp(t.mosfet_c),
        fmt_temp(t.motor_c)
    );
    if let Some(c) = t.mcu_c
        && c >= MCU_TEMP_WARN_C
    {
        warn!("MCU temp elevated: {c:.1}°C (>= {MCU_TEMP_WARN_C:.0}°C)");
    }
    if let Some(c) = t.mosfet_c
        && c >= MOSFET_TEMP_WARN_C
    {
        warn!("MOSFET temp elevated: {c:.1}°C (>= {MOSFET_TEMP_WARN_C:.0}°C)");
    }
    if let Some(c) = t.motor_c
        && c >= MOTOR_TEMP_WARN_C
    {
        warn!("motor temp elevated: {c:.1}°C (>= {MOTOR_TEMP_WARN_C:.0}°C)");
    }
}

fn fmt_temp(t: Option<f32>) -> String {
    t.map_or_else(|| "?".to_string(), |c| format!("{c:.1}°C"))
}

fn print_status(
    game: GameId,
    engine: &EngineState,
    bitmask: u32,
    led_count: usize,
    packets: u32,
    inplace: bool,
) {
    let bar = led_bar(bitmask, led_count);
    let line = format!(
        "[{}] rpm {:>5} / {:>5}  idle {:>4}  [{}]  {} pkt",
        game.label(),
        engine.rpm,
        engine.rpm_redline,
        engine.rpm_idle,
        bar,
        packets
    );
    if inplace {
        // Overwrite a single status line in place; trailing spaces clear
        // any leftover from a previous longer line.
        print!("\r{line}    ");
        let _ = std::io::stdout().flush();
    } else {
        println!("{line}");
    }
}

fn led_bar(bitmask: u32, led_count: usize) -> String {
    (0..led_count)
        .map(|i| {
            if bitmask & (1 << i) != 0 {
                '●'
            } else {
                '○'
            }
        })
        .collect()
}

fn rpm_to_bitmask(engine: &EngineState, led_count: usize) -> u32 {
    use std::sync::OnceLock;

    static RPM_RANGE: OnceLock<(f32, f32)> = OnceLock::new();

    if engine.rpm_redline <= 0 || led_count == 0 {
        return 0;
    }

    let &(start_fraction, full_fraction) = RPM_RANGE.get_or_init(|| {
        let start = std::env::var("MOZA_REV_LED_START")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.70)
            .clamp(0.0, 0.99);

        let full = std::env::var("MOZA_REV_LED_FULL")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.96)
            .clamp(start + 0.01, 1.0);

        (start, full)
    });

    let redline = engine.rpm_redline as f32;
    let start_rpm = redline * start_fraction;
    let full_rpm = redline * full_fraction;
    let rpm = engine.rpm as f32;

    if rpm < start_rpm {
        return 0;
    }

    let fraction = ((rpm - start_rpm) / (full_rpm - start_rpm)).clamp(0.0, 1.0);

    let lit = if led_count == 1 {
        1
    } else {
        1 + (fraction * (led_count - 1) as f32).floor() as usize
    };

    if lit >= 32 {
        u32::MAX
    } else {
        (1u32 << lit) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(rpm: i32) -> EngineState {
        EngineState {
            rpm,
            rpm_redline: 6500,
            rpm_idle: 800,
        }
    }

    #[test]
    fn below_led_band_lights_nothing() {
        // Default LED band starts at 70% of 6500 RPM = 4550 RPM.
        assert_eq!(rpm_to_bitmask(&engine(4500), 10), 0);
    }

    #[test]
    fn start_of_led_band_lights_first_led() {
        assert_eq!(rpm_to_bitmask(&engine(4600), 10), 0x001);
    }

    #[test]
    fn middle_of_led_band_lights_half() {
        // The default 70%-96% band is roughly half traversed near 5400 RPM.
        assert_eq!(rpm_to_bitmask(&engine(5400), 10), 0x01F);
    }

    #[test]
    fn at_redline_lights_everything() {
        assert_eq!(rpm_to_bitmask(&engine(6500), 10), 0x3FF);
    }

    #[test]
    fn above_redline_clamps() {
        assert_eq!(rpm_to_bitmask(&engine(9000), 10), 0x3FF);
    }

    #[test]
    fn unset_engine_data_is_safe() {
        let zero = EngineState {
            rpm: 0,
            rpm_redline: 0,
            rpm_idle: 0,
        };
        assert_eq!(rpm_to_bitmask(&zero, 10), 0);
    }
}
