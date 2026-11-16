//! Just enough NBT to read a few fields out of `level.dat`.

use std::collections::HashMap;
use std::io::Read;

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Str(String),
    Compound(HashMap<String, Tag>),
    Other,
}

impl Tag {
    pub fn get(&self, k: &str) -> Option<&Tag> {
        match self {
            Tag::Compound(m) => m.get(k),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Tag::Byte(v) => Some(*v as i64),
            Tag::Short(v) => Some(*v as i64),
            Tag::Int(v) => Some(*v as i64),
            Tag::Long(v) => Some(*v),
            _ => None,
        }
    }
}

struct R<'a> {
    b: &'a [u8],
    i: usize,
    depth: u32,
}

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn i16(&mut self) -> Option<i16> {
        self.take(2).map(|s| i16::from_be_bytes([s[0], s[1]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.take(4).map(|s| i32::from_be_bytes(s.try_into().unwrap_or([0; 4])))
    }
    fn i64(&mut self) -> Option<i64> {
        self.take(8).map(|s| i64::from_be_bytes(s.try_into().unwrap_or([0; 8])))
    }
    fn string(&mut self) -> Option<String> {
        let n = self.i16()? as u16 as usize;
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn payload(&mut self, t: u8) -> Option<Tag> {
        Some(match t {
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(self.i16()?),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(self.i64()?),
            5 => Tag::Float(f32::from_bits(self.i32()? as u32)),
            6 => Tag::Double(f64::from_bits(self.i64()? as u64)),
            7 => {
                let n = self.i32()?.max(0) as usize;
                self.take(n)?;
                Tag::Other
            }
            8 => Tag::Str(self.string()?),
            9 => {
                let et = self.u8()?;
                let n = self.i32()?.max(0);
                for _ in 0..n {
                    self.payload(et)?;
                }
                Tag::Other
            }
            10 => {
                self.depth += 1;
                if self.depth > 64 {
                    return None;
                }
                let mut m = HashMap::new();
                loop {
                    let ct = self.u8()?;
                    if ct == 0 {
                        break;
                    }
                    let name = self.string()?;
                    m.insert(name, self.payload(ct)?);
                }
                self.depth -= 1;
                Tag::Compound(m)
            }
            11 => {
                let n = self.i32()?.max(0) as usize;
                self.take(n.checked_mul(4)?)?;
                Tag::Other
            }
            12 => {
                let n = self.i32()?.max(0) as usize;
                self.take(n.checked_mul(8)?)?;
                Tag::Other
            }
            _ => return None,
        })
    }
}

/// Parse a (possibly gzipped) NBT file. Returns the root compound.
pub fn parse(data: &[u8]) -> Option<Tag> {
    let raw;
    let bytes = if data.starts_with(&[0x1f, 0x8b]) {
        let mut out = vec![];
        // level.dat is small; cap it so a bad file can't eat memory
        flate2::read::GzDecoder::new(data).take(64 * 1024 * 1024).read_to_end(&mut out).ok()?;
        raw = out;
        &raw[..]
    } else {
        data
    };
    let mut r = R { b: bytes, i: 0, depth: 0 };
    if r.u8()? != 10 {
        return None;
    }
    r.string()?;
    r.payload(10)
}
