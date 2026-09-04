// Automobilista 2 / Project CARS 2 (Madness-engine PC2 UDP) listener.
//
// AMS2 doesn't transmit an idle RPM, so we substitute a typical petrol
// idle so the LED math has a sensible floor. Note: AMS2 broadcasts to
// 255.255.255.255; on Linux the kernel doesn't loop limited-broadcast
// packets back to local sockets - see the README's iptables NAT note.

use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;

use log::{error, info};

use crate::listeners::{EngineState, GameId, Update};
use crate::telemetry::pc2;

const GAME: GameId = GameId::Ams2;

/// AMS2 / PC2 don't transmit an idle RPM. Use a typical petrol-car
/// value; the bar will start lighting slightly above this. Tune via
/// the constant if you race diesels or high-revving race cars.
const ASSUMED_IDLE: i32 = 800;

pub fn spawn(port: u16, tx: mpsc::Sender<Update>) -> bool {
    let bind_addr = format!("0.0.0.0:{port}");
    let socket = match UdpSocket::bind(&bind_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("bind {bind_addr}: {e}");
            return false;
        }
    };
    info!(
        "listening for {} telemetry on udp://{bind_addr}",
        GAME.name()
    );
    thread::Builder::new()
        .name(format!("listener-{}", GAME.label()))
        .spawn(move || run(socket, tx))
        .expect("failed to spawn listener thread");
    true
}

fn run(socket: UdpSocket, tx: mpsc::Sender<Update>) {
    let mut buf = vec![0u8; 2048];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        let Some(engine) = parse(&buf[..n]) else {
            continue;
        };
        if tx.send(Update { game: GAME, engine }).is_err() {
            return;
        }
    }
}

fn parse(buf: &[u8]) -> Option<EngineState> {
    let sample = pc2::decode(buf)?;
    Some(EngineState {
        rpm: sample.rpm,
        rpm_redline: sample.redline_rpm,
        rpm_idle: ASSUMED_IDLE,
    })
}
