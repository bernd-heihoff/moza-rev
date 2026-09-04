#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineSample {
    pub rpm: i32,
    pub redline_rpm: i32,
}

impl EngineSample {
    pub fn rpm_ratio(&self) -> Option<f32> {
        if self.redline_rpm <= 0 {
            return None;
        }

        Some(self.rpm.max(0) as f32 / self.redline_rpm as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_ratio_normalizes_against_redline() {
        let sample = EngineSample {
            rpm: 7_500,
            redline_rpm: 10_000,
        };

        assert_eq!(sample.rpm_ratio(), Some(0.75));
    }

    #[test]
    fn rpm_ratio_clamps_negative_rpm_to_zero() {
        let sample = EngineSample {
            rpm: -1,
            redline_rpm: 10_000,
        };

        assert_eq!(sample.rpm_ratio(), Some(0.0));
    }

    #[test]
    fn rpm_ratio_rejects_missing_redline() {
        let sample = EngineSample {
            rpm: 5_000,
            redline_rpm: 0,
        };

        assert_eq!(sample.rpm_ratio(), None);
    }
}
