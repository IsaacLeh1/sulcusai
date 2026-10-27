// SPDX-License-Identifier: AGPL-3.0-only
//! Reads the header of a GGUF model file: what the model is, how it's
//! built and how it's quantized. Only the key/value header is read; large
//! arrays (the tokenizer's vocabulary) are skipped.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
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

type Reader<R> = BufReader<R>;

/// Skips ahead without dropping the read buffer (vocabularies are hundreds
/// of thousands of short strings).
fn skip<R: Read + Seek>(r: &mut Reader<R>, n: u64) -> std::io::Result<()> {
    r.seek_relative(n as i64)
}

fn string<R: Read + Seek>(r: &mut Reader<R>) -> std::io::Result<Option<String>> {
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

fn scalar<R: Read + Seek>(r: &mut Reader<R>, t: u32) -> std::io::Result<Value> {
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

fn value<R: Read + Seek>(r: &mut Reader<R>, t: u32) -> std::io::Result<Value> {
    if t != 9 {
        return scalar(r, t);
    }
    let item = u32_of(r)?;
    let count = u64_of(r)?;
    match (item, width(item)) {
        // Short arrays of whole numbers and yes/no values (per-layer
        // settings, like which layers slide) are kept, yes as 1.
        (0..=5 | 7 | 10 | 11, Some(_)) if count <= 4096 => {
            let mut out = Vec::with_capacity(count as usize);
            for _ in 0..count {
                out.push(match scalar(r, item)? {
                    Value::Bool(b) => b as i64,
                    v => v.int().unwrap_or(0),
                });
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
    read_from(BufReader::with_capacity(1 << 16, File::open(path)?), false)
}

/// The keys in the first bytes of a GGUF file (the start of a download), as
/// far as they go; fails when not even one is there.
pub fn read_start(bytes: &[u8]) -> std::io::Result<Meta> {
    read_from(BufReader::new(std::io::Cursor::new(bytes)), true)
}

fn read_from<R: Read + Seek>(mut r: Reader<R>, partial: bool) -> std::io::Result<Meta> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(bad("not a GGUF file"));
    }
    let version = u32_of(&mut r)?;
    if !(2..=3).contains(&version) {
        return Err(bad("unsupported GGUF version"));
    }
    let tensors = u64_of(&mut r)?;
    let keys = u64_of(&mut r)?;
    if keys > MAX_KEYS {
        return Err(bad("too many keys"));
    }
    let mut meta = Meta::new();
    for _ in 0..keys {
        let next = (|| -> std::io::Result<(String, Value)> {
            let key = string(&mut r)?.ok_or_else(|| bad("key too long"))?;
            let t = u32_of(&mut r)?;
            Ok((key, value(&mut r, t)?))
        })();
        match next {
            Ok((k, v)) => {
                meta.insert(k, v);
            }
            Err(_) if partial && !meta.is_empty() => return Ok(meta),
            Err(e) => return Err(e),
        }
    }
    if partial {
        return Ok(meta);
    }
    // The weights' own type, for files whose header names another (some
    // conversions say F16 while most weights are MXFP4).
    if tensors <= MAX_TENSORS {
        if let Ok(Some(t)) = main_tensor_type(&mut r, tensors) {
            meta.insert(MAIN_TYPE_KEY.into(), Value::Int(t as i64));
        }
    }
    Ok(meta)
}

/// A tensor in a GGUF file and where its data starts in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub name: String,
    pub dims: Vec<u64>,
    /// ggml's type number.
    pub kind: u32,
    pub start: u64,
}

/// The file's tensors (the list after the keys), each with where its data
/// starts: the data begins at the next multiple of the file's alignment.
pub fn tensors(path: &Path) -> std::io::Result<Vec<Tensor>> {
    let mut r = BufReader::with_capacity(1 << 16, File::open(path)?);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(bad("not a GGUF file"));
    }
    if !(2..=3).contains(&u32_of(&mut r)?) {
        return Err(bad("unsupported GGUF version"));
    }
    let n = u64_of(&mut r)?;
    let keys = u64_of(&mut r)?;
    if keys > MAX_KEYS || n > MAX_TENSORS {
        return Err(bad("too many keys or tensors"));
    }
    let mut align = 32u64;
    for _ in 0..keys {
        let key = string(&mut r)?.unwrap_or_default();
        let t = u32_of(&mut r)?;
        let v = value(&mut r, t)?;
        if key == "general.alignment" {
            align = v.int().filter(|a| *a > 0).unwrap_or(32) as u64;
        }
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let name = string(&mut r)?.ok_or_else(|| bad("tensor name too long"))?;
        let d = u32_of(&mut r)?;
        if d > 8 {
            return Err(bad("too many dimensions"));
        }
        let mut dims = Vec::with_capacity(d as usize);
        for _ in 0..d {
            dims.push(u64_of(&mut r)?);
        }
        let kind = u32_of(&mut r)?;
        let offset = u64_of(&mut r)?;
        out.push(Tensor { name, dims, kind, start: offset });
    }
    let data = r.stream_position()?.div_ceil(align) * align;
    for t in &mut out {
        t.start += data;
    }
    Ok(out)
}

/// Where `read` puts the type most of the weights have (ggml's number).
pub const MAIN_TYPE_KEY: &str = "sulcusai.main_tensor_type";
const MAX_TENSORS: u64 = 100_000;

/// The tensor type covering the most weights, from the tensor list that
/// follows the keys (name, dimensions, type, offset for each).
fn main_tensor_type<R: Read + Seek>(r: &mut Reader<R>, n: u64) -> std::io::Result<Option<u32>> {
    let mut by_type: HashMap<u32, u128> = HashMap::new();
    for _ in 0..n {
        string(r)?;
        let dims = u32_of(r)?;
        if dims > 8 {
            return Err(bad("too many dimensions"));
        }
        let mut count: u128 = 1;
        for _ in 0..dims {
            count = count.saturating_mul(u64_of(r)? as u128);
        }
        let t = u32_of(r)?;
        u64_of(r)?;
        *by_type.entry(t).or_default() += count;
    }
    Ok(by_type.into_iter().max_by_key(|&(_, c)| c).map(|(t, _)| t))
}

/// A quantized ggml tensor type as the closest name the app uses (None for
/// full-precision types, where the header's own name is right).
pub fn tensor_quant_name(t: i64) -> Option<&'static str> {
    Some(match t {
        2 => "Q4_0",
        3 => "Q4_1",
        6 => "Q5_0",
        7 => "Q5_1",
        8 => "Q8_0",
        10 => "Q2_K",
        11 => "Q3_K_M",
        12 => "Q4_K_M",
        13 => "Q5_K_M",
        14 => "Q6_K",
        18 => "IQ3_XXS",
        20 => "IQ4_NL",
        21 => "IQ3_S",
        23 => "IQ4_XS",
        39 => "MXFP4",
        _ => return None,
    })
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
        assert!(!m.contains_key(MAIN_TYPE_KEY), "no tensors listed");
        // The start of a download: the keys as far as they go.
        assert_eq!(read_start(&f).unwrap()["general.architecture"].str(), Some("qwen2"));
        let cut = read_start(&f[..75]).unwrap();
        assert_eq!((cut["general.architecture"].str(), cut.len()), (Some("qwen2"), 1));
        assert!(read_start(&f[..40]).is_err(), "not even the first key");
    }

    /// A header that says F16 over weights that are mostly MXFP4 (a
    /// gpt-oss conversion did this): the tensor list tells the truth.
    #[test]
    fn finds_the_type_most_weights_have() {
        let mut f = b"GGUF".to_vec();
        f.extend(3u32.to_le_bytes());
        f.extend(2u64.to_le_bytes());
        f.extend(1u64.to_le_bytes());
        s(&mut f, "general.file_type");
        f.extend(4u32.to_le_bytes());
        f.extend(1u32.to_le_bytes());
        // (name, dimensions, ggml type, offset)
        for (name, dims, t) in [("token_embd.weight", vec![64u64, 32], 1u32), ("blk.0.ffn_down_exps.weight", vec![64, 32, 8], 39)] {
            s(&mut f, name);
            f.extend((dims.len() as u32).to_le_bytes());
            for d in dims {
                f.extend(d.to_le_bytes());
            }
            f.extend(t.to_le_bytes());
            f.extend(0u64.to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!("sulcusai-gguf-{}.gguf", uuid::Uuid::new_v4()));
        std::fs::File::create(&path).unwrap().write_all(&f).unwrap();
        let m = read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(quant_name(m["general.file_type"].int().unwrap()), "F16");
        assert_eq!(m[MAIN_TYPE_KEY].int().and_then(tensor_quant_name), Some("MXFP4"));
    }

    #[test]
    fn rejects_other_files() {
        let path = std::env::temp_dir().join(format!("sulcusai-gguf-{}.bin", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"not a model at all").unwrap();
        assert!(read(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
