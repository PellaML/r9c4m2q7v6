use crate::{
    json::Json,
    plan::{Case, POISON, STAMP_XOR},
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub rgba: [u32; 4],
    pub stamp: u32,
    pub reserved: [u32; 3],
}
impl Sample {
    pub fn poison() -> Self {
        Self {
            rgba: [POISON; 4],
            stamp: 0xdead_beef,
            reserved: [POISON; 3],
        }
    }
}

pub const BLACK: [u32; 4] = [0, 0, 0, 0x3f80_0000];
pub const WHITE: [u32; 4] = [0x3f80_0000; 4];
pub const DELIBERATELY_WRONG_WHITE: [u32; 4] = [0; 4];

pub fn bits_json(bits: &[u32]) -> Json {
    Json::Array(
        bits.iter()
            .map(|v| Json::from(format!("{v:08x}")))
            .collect(),
    )
}

pub struct Checks {
    pub valid: bool,
    pub bad_stamps: usize,
    pub poison_channels: usize,
    pub nonfinite_channels: usize,
    pub nonzero_channels: usize,
    pub center_valid: bool,
    pub center: [u32; 4],
}

pub fn examine(case: &Case, samples: &[Sample]) -> Checks {
    let mut result = Checks {
        valid: false,
        bad_stamps: 0,
        poison_channels: 0,
        nonfinite_channels: 0,
        nonzero_channels: 0,
        center_valid: false,
        center: [POISON; 4],
    };
    for (index, sample) in samples.iter().enumerate() {
        if sample.stamp != case.token() ^ index as u32 ^ STAMP_XOR || sample.reserved != [0; 3] {
            result.bad_stamps += 1;
        }
        for bits in sample.rgba {
            let v = f32::from_bits(bits);
            result.poison_channels += usize::from(bits == POISON);
            result.nonfinite_channels += usize::from(!v.is_finite());
            result.nonzero_channels += usize::from(v.is_finite() && v != 0.0);
        }
    }
    if let Some(center) = samples.first() {
        result.center = center.rgba;
        result.center_valid = match case.color {
            0 => center.rgba == BLACK,
            1 => center.rgba == WHITE,
            2 => center.rgba == [0; 4],
            _ => center.rgba.iter().all(|b| {
                let v = f32::from_bits(*b);
                v.is_finite() && v > 0.0 && v <= 1.0
            }),
        };
    }
    // Far-coordinate nonfinite values are reported, not given an invented oracle.
    result.valid = samples.len() == case.count
        && result.bad_stamps == 0
        && result.poison_channels == 0
        && result.center_valid;
    result
}

impl Checks {
    pub fn json(&self) -> Json {
        Json::object([
            ("passed", Json::from(self.valid)),
            ("bad_stamps", Json::from(self.bad_stamps)),
            ("poison_channels", Json::from(self.poison_channels)),
            ("nonfinite_channels", Json::from(self.nonfinite_channels)),
            ("nonzero_channels", Json::from(self.nonzero_channels)),
            ("center_anchor_passed", Json::from(self.center_valid)),
            ("center_rgba_f32_bits", bits_json(&self.center)),
        ])
    }
}

pub fn wrong_oracle_report(observed: [u32; 4]) -> (i32, Json) {
    let matched = observed == DELIBERATELY_WRONG_WHITE;
    let exit = if matched { 0 } else { 1 };
    (
        exit,
        Json::object([
            ("control", Json::from("deliberately_wrong_white_center")),
            (
                "expected_rgba_f32_bits",
                bits_json(&DELIBERATELY_WRONG_WHITE),
            ),
            ("observed_rgba_f32_bits", bits_json(&observed)),
            ("comparison_matched", Json::from(matched)),
            ("exit_code", Json::from(exit)),
            ("required_exit_code", Json::from(1_i32)),
            ("rejected_as_required", Json::from(!matched)),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cohort, plan};
    #[test]
    fn output_abi_matches_shader_storage() {
        assert_eq!(std::mem::size_of::<Sample>(), 32);
        assert_eq!(std::mem::offset_of!(Sample, stamp), 16);
    }
    #[test]
    fn invalid_control_is_a_real_comparison_not_a_hardcoded_exit() {
        assert_eq!(wrong_oracle_report(WHITE).0, 1);
        assert_eq!(wrong_oracle_report(DELIBERATELY_WRONG_WHITE).0, 0);
    }
    #[test]
    fn missing_and_stale_writes_fail_even_with_correct_white_color() {
        let mut case = plan::cases(&cohort::generate())[1];
        case.count = 1;
        let mut sample = Sample {
            rgba: WHITE,
            stamp: case.token() ^ STAMP_XOR,
            reserved: [0; 3],
        };
        assert!(examine(&case, &[sample]).valid);
        sample.stamp ^= 1;
        assert!(!examine(&case, &[sample]).valid);
        assert!(!examine(&case, &[Sample::poison()]).valid);
        assert!(!examine(&case, &[]).valid);
    }
    #[test]
    fn black_and_transparent_black_are_distinct() {
        let mut case = plan::cases(&cohort::generate())[0];
        case.count = 1;
        let mut sample = Sample {
            rgba: BLACK,
            stamp: case.token() ^ STAMP_XOR,
            reserved: [0; 3],
        };
        assert!(examine(&case, &[sample]).valid);
        sample.rgba = [0; 4];
        assert!(!examine(&case, &[sample]).valid);
    }
}
