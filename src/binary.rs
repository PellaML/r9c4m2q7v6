use crate::{
    checks::{Sample, BLACK},
    cohort::Cohort,
    plan::{Case, STAMP_XOR},
};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

const COORD_MAGIC: &[u8; 8] = b"VPCOOR01";
const SAMPLE_MAGIC: &[u8; 8] = b"VPSAMP01";
const ANCHOR_MAGIC: &[u8; 8] = b"VPANCH01";

fn word(writer: &mut impl Write, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}
fn header(writer: &mut impl Write, magic: &[u8; 8], stride: u32, count: u32) -> io::Result<()> {
    writer.write_all(magic)?;
    word(writer, stride)?;
    word(writer, count)
}

pub fn write_coordinates(path: &Path, cohort: &Cohort) -> io::Result<()> {
    let mut f = io::BufWriter::new(File::create(path)?);
    header(&mut f, COORD_MAGIC, 12, cohort.coordinates.len() as u32)?;
    for c in &cohort.coordinates {
        for v in c {
            word(&mut f, *v)?;
        }
    }
    f.flush()
}

pub struct Samples {
    writer: io::BufWriter<File>,
    pub count: u32,
}
impl Samples {
    pub fn new(path: &Path) -> io::Result<Self> {
        let mut writer = io::BufWriter::new(File::create(path)?);
        header(&mut writer, SAMPLE_MAGIC, 20, 0)?;
        Ok(Self { writer, count: 0 })
    }
    pub fn append(&mut self, samples: &[Sample]) -> io::Result<u32> {
        let first = self.count;
        for s in samples {
            for v in s.rgba {
                word(&mut self.writer, v)?;
            }
            word(&mut self.writer, s.stamp)?;
        }
        self.count = self
            .count
            .checked_add(samples.len() as u32)
            .ok_or_else(|| io::Error::other("sample count overflow"))?;
        Ok(first)
    }
    pub fn finish(mut self) -> io::Result<u32> {
        self.writer.flush()?;
        self.writer.seek(SeekFrom::Start(12))?;
        word(&mut self.writer, self.count)?;
        self.writer.flush()?;
        Ok(self.count)
    }
}

#[derive(Clone, Copy)]
pub struct Anchor {
    pub case_id: u32,
    pub coordinate_index: u32,
    pub sample: Sample,
    pub token: u32,
}
impl Anchor {
    pub fn from_case(case: &Case, sample: Sample) -> Self {
        Self {
            case_id: case.id,
            coordinate_index: 0,
            sample,
            token: case.token(),
        }
    }
}

pub fn write_anchors(path: &Path, anchors: &[Anchor; 2]) -> io::Result<()> {
    let mut writer = File::create(path)?;
    header(&mut writer, ANCHOR_MAGIC, 32, 2)?;
    for a in anchors {
        word(&mut writer, a.case_id)?;
        word(&mut writer, a.coordinate_index)?;
        for v in a.sample.rgba {
            word(&mut writer, v)?;
        }
        word(&mut writer, a.sample.stamp)?;
        word(&mut writer, a.token)?;
    }
    writer.sync_all()
}

pub fn read_white_anchor(path: &Path) -> io::Result<[u32; 4]> {
    let mut bytes = Vec::new();
    File::open(path)?.take(81).read_to_end(&mut bytes)?;
    decode_white_anchor(&bytes)
}

fn decode_white_anchor(bytes: &[u8]) -> io::Result<[u32; 4]> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "invalid anchor table");
    if bytes.len() != 80 || &bytes[..8] != ANCHOR_MAGIC {
        return Err(bad());
    }
    let read = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    if read(8) != 32 || read(12) != 2 {
        return Err(bad());
    }
    for i in 0..2 {
        let start = 16 + i * 32;
        if read(start) != i as u32
            || read(start + 4) != 0
            || read(start + 28) != 0xc001_0000 ^ i as u32
            || read(start + 24) != read(start + 28) ^ STAMP_XOR
        {
            return Err(bad());
        }
    }
    let black = std::array::from_fn::<_, 4, _>(|i| read(24 + i * 4));
    if black != BLACK {
        return Err(bad());
    }
    Ok(std::array::from_fn(|i| read(56 + i * 4)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::WHITE;
    #[test]
    fn anchor_fixture_validation_rejects_corruption() {
        // Format-only test data; no synthetic observations are produced by run mode.
        let mut bytes = Vec::new();
        header(&mut bytes, ANCHOR_MAGIC, 32, 2).unwrap();
        for (id, rgba) in [BLACK, WHITE].into_iter().enumerate() {
            let token = 0xc001_0000 ^ id as u32;
            word(&mut bytes, id as u32).unwrap();
            word(&mut bytes, 0).unwrap();
            for v in rgba {
                word(&mut bytes, v).unwrap();
            }
            word(&mut bytes, token ^ STAMP_XOR).unwrap();
            word(&mut bytes, token).unwrap();
        }
        assert_eq!(decode_white_anchor(&bytes).unwrap(), WHITE);
        let mut corrupt = bytes.clone();
        corrupt[72] ^= 1;
        assert!(decode_white_anchor(&corrupt).is_err());
        assert!(decode_white_anchor(&bytes[..79]).is_err());
        bytes.push(0);
        assert!(decode_white_anchor(&bytes).is_err());
    }
}
