// SPDX-License-Identifier: AGPL-3.0-only
//! Reads the header of a GGUF model file: what the model is, how it's
//! built and how it's quantized. Only the key/value header is read; large
//! arrays (the tokenizer's vocabulary) are skipped.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    /// Arrays of numbers keep their values (some models give one value per
    /// layer); other arrays only their length.
    Ints(Vec<i64>),
    Array(u64),
}

impl Value {
    pub fn int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            Value::Ints(v) => v.iter().copied().max(),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

pub type Meta = HashMap<String, Value>;

/// Strings longer than this are dropped (chat templates are a few KB).
const MAX_STRING: u64 = 1 << 20;
const MAX_KEYS: u64 = 10_000;

fn u32_of<R: Read>(r: &mut R) -> std::io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn u64_of<R: Read>(r: &mut R) -> std::io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn bad(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

type Reader = BufReader<File>;

/// Skips ahead without dropping the read buffer (vocabularies are hundreds
/// of thousands of short strings).
fn skip(r: &mut Reader, n: u64) -> std::io::Result<()> {
    r.seek_relative(n as i64)
}

fn string(r: &mut Reader) -> std::io::Result<Option<String>> {
    let len = u64_of(r)?;
    if len > MAX_STRING {
        skip(r, len)?;
        return Ok(None);
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Byte width of a fixed-size value type, if it is one.
fn width(t: u32) -> Option<i64> {
    match t {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4 | 5 | 6 => Some(4),
        10 | 11 | 12 => Some(8),
        _ => None,
    }
}

fn scalar(r: &mut Reader, t: u32) -> std::io::Result<Value> {
    let mut b = [0u8; 8];
    Ok(match t {
        0 => {
            r.read_exact(&mut b[..1])?;
            Value::Int(b[0] as i64)
        }
        1 => {
            r.read_exact(&mut b[..1])?;
            Value::Int(b[0] as i8 as i64)
        }
        2 => {
            r.read_exact(&mut b[..2])?;
            Value::Int(u16::from_le_bytes([b[0], b[1]]) as i64)
        }
        3 => {
            r.read_exact(&mut b[..2])?;
            Value::Int(i16::from_le_bytes([b[0], b[1]]) as i64)
        }
        4 => Value::Int(u32_of(r)? as i64),
        5 => Value::Int(u32_of(r)? as i32 as i64),
        6 => Value::Float(f32::from_bits(u32_of(r)?) as f64),
        7 => {
            r.read_exact(&mut b[..1])?;
            Value::Bool(b[0] != 0)
        }
        8 => string(r)?.map_or(Value::Array(0), Value::Str),
        10 => Value::Int(u64_of(r)? as i64),
        11 => Value::Int(u64_of(r)? as i64),
        12 => Value::Float(f64::from_bits(u64_of(r)?)),
        _ => return Err(bad("unknown value type")),
    })
}

fn value(r: &mut Reader, t: u32) -> std::io::Result<Value> {
    if t != 9 {
        return scalar(r, t);
    }
    let item = u32_of(r)?;
    let count = u64_of(r)?;
    match (item, width(item)) {
        // Short arrays of whole numbers (per-layer settings) are kept.
        (0..=5 | 10 | 11, Some(_)) if count <= 4096 => {
            let mut out = Vec::with_capacity(count as usize);
            for _ in 0..count {
                out.push(scalar(r, item)?.int().unwrap_or(0));
            }
            Ok(Value::Ints(out))
        }
        (_, Some(w)) => {
            skip(r, w as u64 * count)?;
            Ok(Value::Array(count))
        }
        (8, None) => {
            for _ in 0..count {
                let len = u64_of(r)?;
                skip(r, len)?;
            }
            Ok(Value::Array(count))
        }
        _ => Err(bad("nested arrays are not supported")),
    }
}

pub fn read(path: &Path) -> std::io::Result<Meta> {
    let mut r = BufReader::with_capacity(1 << 16, File::open(path)?);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(bad("not a GGUF file"));
    }
    let version = u32_of(&mut r)?;
    if !(2..=3).contains(&version) {
        return Err(bad("unsupported GGUF version"));
    }
    let _tensors = u64_of(&mut r)?;
    let keys = u64_of(&mut r)?;
    if keys > MAX_KEYS {
        return Err(bad("too many keys"));
    }
    let mut meta = Meta::new();
    for _ in 0..keys {
        let key = string(&mut r)?.ok_or_else(|| bad("key too long"))?;
        let t = u32_of(&mut r)?;
        meta.insert(key, value(&mut r, t)?);
    }
    Ok(meta)
}

/// llama.cpp's file type numbers, as the quantization names the app uses.
pub fn quant_name(file_type: i64) -> &'static str {
    match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4",
        _ => "Unknown",
    }
}

/// Bits per weight, roughly, for estimating a model's size in parameters.
pub fn bits_per_weight(quant: &str) -> f64 {
    match quant {
        "F32" => 32.0,
        "F16" | "BF16" => 16.0,
        "Q8_0" => 8.5,
        "Q6_K" => 6.6,
        "Q5_K_M" | "Q5_K_S" | "Q5_0" | "Q5_1" => 5.6,
        "Q4_K_M" | "Q4_K_S" | "Q4_0" | "Q4_1" | "IQ4_NL" | "IQ4_XS" | "MXFP4" => 4.8,
        "Q3_K_L" | "Q3_K_M" | "Q3_K_S" | "IQ3_S" | "IQ3_M" | "IQ3_XS" | "IQ3_XXS" => 3.9,
        _ => 2.8,
    }
}

/// The app's quality words for a quantization.
pub fn quality(quant: &str) -> &'static str {
    match quant {
        "F32" | "F16" | "BF16" | "Q8_0" => "Best",
        "Q6_K" => "Great",
        "Q5_K_M" | "Q5_K_S" | "Q5_0" | "Q5_1" => "Better",
        "Q4_K_M" | "Q4_K_S" | "Q4_0" | "Q4_1" | "IQ4_NL" | "IQ4_XS" | "MXFP4" => "Good",
        _ => "Small",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn s(out: &mut Vec<u8>, v: &str) {
        out.extend((v.len() as u64).to_le_bytes());
        out.extend(v.as_bytes());
    }

    #[test]
    fn reads_keys_and_skips_big_arrays() {
        let mut f = b"GGUF".to_vec();
        f.extend(3u32.to_le_bytes());
        f.extend(0u64.to_le_bytes());
        f.extend(5u64.to_le_bytes());
        s(&mut f, "general.architecture");
        f.extend(8u32.to_le_bytes());
        s(&mut f, "qwen2");
        s(&mut f, "qwen2.block_count");
        f.extend(4u32.to_le_bytes());
        f.extend(48u32.to_le_bytes());
        s(&mut f, "tokenizer.ggml.tokens");
        f.extend(9u32.to_le_bytes());
        f.extend(8u32.to_le_bytes());
        f.extend(2u64.to_le_bytes());
        s(&mut f, "hello");
        s(&mut f, "world");
        s(&mut f, "per_layer");
        f.extend(9u32.to_le_bytes());
        f.extend(5u32.to_le_bytes());
        f.extend(2u64.to_le_bytes());
        f.extend(4i32.to_le_bytes());
        f.extend(8i32.to_le_bytes());
        s(&mut f, "general.file_type");
        f.extend(4u32.to_le_bytes());
        f.extend(15u32.to_le_bytes());

        let path = std::env::temp_dir().join(format!("sulcusai-gguf-{}.gguf", uuid::Uuid::new_v4()));
        std::fs::File::create(&path).unwrap().write_all(&f).unwrap();
        let m = read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(m["general.architecture"].str(), Some("qwen2"));
        assert_eq!(m["qwen2.block_count"].int(), Some(48));
        assert_eq!(m["tokenizer.ggml.tokens"], Value::Array(2));
        assert_eq!(m["per_layer"].int(), Some(8));
        assert_eq!(quant_name(m["general.file_type"].int().unwrap()), "Q4_K_M");
    }

    #[test]
    fn rejects_other_files() {
        let path = std::env::temp_dir().join(format!("sulcusai-gguf-{}.bin", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"not a model at all").unwrap();
        assert!(read(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
