//! WAL delta entry encoding for symbols and edges.

use anyhow::{Result, bail};

fn push_u16_len_prefixed(buf: &mut Vec<u8>, bytes: &[u8], field: &str) -> Result<()> {
    if bytes.len() > u16::MAX as usize {
        bail!("delta {field} length {} exceeds u16::MAX", bytes.len());
    }
    buf.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(bytes);
    Ok(())
}

pub fn encode_symbol(name: &str, kind: u8, tier: u8, start: u32, end: u32) -> Result<Vec<u8>> {
    let mut p = Vec::with_capacity(2 + name.len() + 10);
    push_u16_len_prefixed(&mut p, name.as_bytes(), "symbol name")?;
    p.push(kind);
    p.push(tier);
    p.extend_from_slice(&start.to_le_bytes());
    p.extend_from_slice(&end.to_le_bytes());
    Ok(p)
}

pub fn decode_symbol(p: &[u8]) -> Option<(String, (u8, u8, u32, u32))> {
    if p.len() < 2 {
        return None;
    }
    let len = u16::from_le_bytes(p[0..2].try_into().ok()?) as usize;
    if p.len() < 2 + len + 10 {
        return None;
    }
    let name = String::from_utf8(p[2..2 + len].to_vec()).ok()?;
    let at = 2 + len;
    let kind = p[at];
    let tier = p[at + 1];
    let start = u32::from_le_bytes(p[at + 2..at + 6].try_into().ok()?);
    let end = u32::from_le_bytes(p[at + 6..at + 10].try_into().ok()?);
    Some((name, (kind, tier, start, end)))
}

pub fn encode_edge(
    src: &str,
    dst: &str,
    kind: u8,
    conf: u8,
    start: u32,
    end: u32,
) -> Result<Vec<u8>> {
    let mut p = Vec::new();
    push_u16_len_prefixed(&mut p, src.as_bytes(), "edge src")?;
    push_u16_len_prefixed(&mut p, dst.as_bytes(), "edge dst")?;
    p.push(kind);
    p.push(conf);
    p.extend_from_slice(&start.to_le_bytes());
    p.extend_from_slice(&end.to_le_bytes());
    Ok(p)
}

pub fn encode_edge_with_meta(
    src: &str,
    dst: &str,
    kind: u8,
    conf: u8,
    start: u32,
    end: u32,
    source: Option<&str>,
) -> Result<Vec<u8>> {
    let mut p = encode_edge(src, dst, kind, conf, start, end)?;
    if let Some(s) = source {
        push_u16_len_prefixed(&mut p, s.as_bytes(), "edge source")?;
    }
    Ok(p)
}

fn read_u16_le(p: &[u8], at: &mut usize) -> Option<usize> {
    let len = u16::from_le_bytes(p.get(*at..*at + 2)?.try_into().ok()?) as usize;
    *at += 2;
    Some(len)
}

fn read_utf8_field(p: &[u8], at: &mut usize, len: usize) -> Option<String> {
    let s = String::from_utf8(p.get(*at..*at + len)?.to_vec()).ok()?;
    *at += len;
    Some(s)
}

fn read_u32_le(p: &[u8], at: &mut usize) -> Option<u32> {
    let v = u32::from_le_bytes(p.get(*at..*at + 4)?.try_into().ok()?);
    *at += 4;
    Some(v)
}

fn read_optional_source(p: &[u8], at: usize) -> Option<String> {
    if at >= p.len() {
        return None;
    }
    let mut pos = at;
    let slen = read_u16_le(p, &mut pos)?;
    read_utf8_field(p, &mut pos, slen)
}

#[allow(clippy::type_complexity)]
pub fn decode_edge(p: &[u8]) -> Option<(String, String, u8, u8, u32, u32, Option<String>)> {
    let mut at = 0usize;
    let src_len = read_u16_le(p, &mut at)?;
    let src = read_utf8_field(p, &mut at, src_len)?;
    let dst_len = read_u16_le(p, &mut at)?;
    let dst = read_utf8_field(p, &mut at, dst_len)?;
    let kind = *p.get(at)?;
    let conf = *p.get(at + 1)?;
    at += 2;
    let start = read_u32_le(p, &mut at)?;
    let end = read_u32_le(p, &mut at)?;
    let source = read_optional_source(p, at);
    Some((src, dst, kind, conf, start, end, source))
}
