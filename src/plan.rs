use crate::cohort::Cohort;

pub const COLORS: [[u8; 4]; 8] = [
    [0, 0, 0, 255],
    [255, 255, 255, 255],
    [0, 0, 0, 0],
    [17, 67, 137, 223],
    [251, 109, 43, 181],
    [29, 241, 157, 61],
    [193, 47, 229, 113],
    [7, 131, 71, 239],
];
pub const FOCAL_MASKS: [u32; 4] = [0, 7, 4, 3];
pub const STAMP_XOR: u32 = 0x9e37_79b9;
pub const POISON: u32 = 0x7fc1_2345;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    Level0Float,
    Level0Half,
    ImplicitFloat,
    BiasFloat,
    ImplicitHalf,
    BiasHalf,
}
impl Kernel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Level0Float => "read3d_level0_float",
            Self::Level0Half => "read3d_level0_half",
            Self::ImplicitFloat => "read3d_implicit_float",
            Self::BiasFloat => "read3d_bias_float",
            Self::ImplicitHalf => "read3d_implicit_half",
            Self::BiasHalf => "read3d_bias_half",
        }
    }
    pub fn level0(self) -> bool {
        matches!(self, Self::Level0Float | Self::Level0Half)
    }
}
pub const KERNELS: [Kernel; 6] = [
    Kernel::Level0Float,
    Kernel::Level0Half,
    Kernel::ImplicitFloat,
    Kernel::BiasFloat,
    Kernel::ImplicitHalf,
    Kernel::BiasHalf,
];

#[derive(Clone, Copy)]
pub struct Case {
    pub id: u32,
    pub kernel: Kernel,
    pub linear: bool,
    pub zero_axes: u32,
    pub pixel: bool,
    pub constexpr: bool,
    pub color: usize,
    pub count: usize,
    pub dense: bool,
}
impl Case {
    pub fn sampler_id(&self) -> u32 {
        self.zero_axes
            | ((self.linear as u32) << 3)
            | ((self.pixel as u32) << 4)
            | ((self.constexpr as u32) << 5)
    }
    pub fn token(&self) -> u32 {
        0xc001_0000 ^ self.id
    }
    pub fn required(&self) -> bool {
        !self.pixel && !self.constexpr
    }
    pub fn module_name(&self) -> String {
        format!(
            "{}{}",
            self.kernel.name(),
            if self.constexpr {
                if self.pixel {
                    "_pixel_constexpr"
                } else {
                    "_constexpr"
                }
            } else {
                ""
            }
        )
    }
    pub fn anchor(&self) -> bool {
        self.kernel == Kernel::Level0Float
            && !self.linear
            && self.zero_axes == 0
            && !self.pixel
            && !self.constexpr
            && self.color < 2
    }
}

pub fn cases(cohort: &Cohort) -> Vec<Case> {
    let mut result = Vec::new();
    // Mandatory normalized runtime cases precede every optional case.
    for (pixel, constexpr) in [(false, false), (false, true), (true, false), (true, true)] {
        for kernel in KERNELS {
            if (pixel || constexpr) && !kernel.level0() {
                continue;
            }
            for linear in [false, true] {
                for zero_axes in 0..8 {
                    if !kernel.level0() && !FOCAL_MASKS.contains(&zero_axes) {
                        continue;
                    }
                    let dense =
                        kernel.level0() && !pixel && linear && FOCAL_MASKS.contains(&zero_axes);
                    for color in 0..COLORS.len() {
                        result.push(Case {
                            id: result.len() as u32,
                            kernel,
                            linear,
                            zero_axes,
                            pixel,
                            constexpr,
                            color,
                            dense,
                            count: if dense {
                                cohort.coordinates.len()
                            } else {
                                cohort.core_count
                            },
                        });
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_matrix_and_fixed_anchors() {
        let p = cases(&crate::cohort::generate());
        assert_eq!(p.len(), 1280);
        assert_eq!(p.iter().filter(|c| c.required()).count(), 512);
        assert_eq!(p.iter().filter(|c| c.dense).count(), 128);
        assert!(p[0].anchor() && p[1].anchor());
        assert!(p.iter().enumerate().all(|(i, c)| c.id as usize == i));
        assert!(p.iter().skip(512).all(|c| !c.required()));
        for mask in 0..8 {
            assert!(p.iter().any(|c| c.zero_axes == mask));
        }
        assert!(p.iter().map(|c| c.count * 20).sum::<usize>() < 45_000_000);
    }
    #[test]
    fn mixed_and_independent_address_modes() {
        let p = cases(&crate::cohort::generate());
        for mask in [0, 1, 2, 3, 4, 5, 6, 7] {
            for pixel in [false, true] {
                for constexpr in [false, true] {
                    assert!(p.iter().any(|c| c.zero_axes == mask
                        && c.pixel == pixel
                        && c.constexpr == constexpr
                        && c.linear));
                }
            }
        }
        assert!(COLORS[3..].iter().flatten().all(|v| *v > 0));
    }
}
