use crate::madness::{self, TelemetryPacket};

use super::engine::EngineSample;

pub const DEFAULT_PORT: u16 = madness::DEFAULT_PORT;

/// Decode one Project CARS 2 / Madness telemetry datagram into the normalized
/// engine data used by the LED policy layer.
///
/// Transport ownership deliberately lives outside this module. Callers may
/// receive datagrams from a UDP listener, a bridge, a replay, or a test.
pub fn decode(data: &[u8]) -> Option<EngineSample> {
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
            decode(&data),
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

        assert_eq!(decode(&data), None);
    }

    #[test]
    fn ignores_short_datagrams() {
        assert_eq!(decode(&[0; 32]), None);
    }

    #[test]
    fn ignores_samples_without_redline() {
        let data = telemetry_datagram(7_500, 0);

        assert_eq!(decode(&data), None);
    }
}
