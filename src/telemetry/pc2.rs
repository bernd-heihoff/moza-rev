use std::io;
use std::net::UdpSocket;
use std::time::Duration;

use crate::madness::{self, TelemetryPacket};

use super::engine::EngineSample;

pub const DEFAULT_PORT: u16 = madness::DEFAULT_PORT;
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_millis(20);
const BUFFER_SIZE: usize = 2048;

pub struct Pc2Adapter {
    socket: UdpSocket,
    buffer: [u8; BUFFER_SIZE],
}

impl Pc2Adapter {
    pub fn bind(port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))?;
        socket.set_read_timeout(Some(DEFAULT_READ_TIMEOUT))?;

        Ok(Self {
            socket,
            buffer: [0; BUFFER_SIZE],
        })
    }

    pub fn recv(&mut self) -> io::Result<Option<EngineSample>> {
        match self.socket.recv(&mut self.buffer) {
            Ok(length) => Ok(sample_from_datagram(&self.buffer[..length])),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

fn sample_from_datagram(data: &[u8]) -> Option<EngineSample> {
    let packet = TelemetryPacket::from_bytes(data)?;
    let redline_rpm = packet.data.redline_rpm();

    if redline_rpm <= 0 {
        return None;
    }

    Some(EngineSample {
        rpm: packet.data.rpm(),
        redline_rpm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::madness::{PACKET_TYPE_RACE, PACKET_TYPE_TELEMETRY, TELEMETRY_PACKET_BYTES};

    const RPM_OFFSET: usize = madness::HEADER_BYTES + 28;
    const REDLINE_OFFSET: usize = madness::HEADER_BYTES + 30;

    fn telemetry_datagram(rpm: u16, redline_rpm: u16) -> Vec<u8> {
        let mut data = vec![0; TELEMETRY_PACKET_BYTES];
        data[10] = PACKET_TYPE_TELEMETRY;
        data[RPM_OFFSET..RPM_OFFSET + 2].copy_from_slice(&rpm.to_le_bytes());
        data[REDLINE_OFFSET..REDLINE_OFFSET + 2].copy_from_slice(&redline_rpm.to_le_bytes());
        data
    }

    #[test]
    fn converts_pc2_telemetry_to_engine_sample() {
        let data = telemetry_datagram(7_500, 10_000);

        assert_eq!(
            sample_from_datagram(&data),
            Some(EngineSample {
                rpm: 7_500,
                redline_rpm: 10_000,
            })
        );
    }

    #[test]
    fn ignores_non_telemetry_packets() {
        let mut data = telemetry_datagram(7_500, 10_000);
        data[10] = PACKET_TYPE_RACE;

        assert_eq!(sample_from_datagram(&data), None);
    }

    #[test]
    fn ignores_short_datagrams() {
        assert_eq!(sample_from_datagram(&[0; 32]), None);
    }

    #[test]
    fn ignores_samples_without_redline() {
        let data = telemetry_datagram(7_500, 0);

        assert_eq!(sample_from_datagram(&data), None);
    }
}
