use zero_abi::SOURCE_BYTE_LIMIT;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedCell {
    source: String,
    digest: String,
}

impl PreparedCell {
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Clone, Debug, Default)]
pub struct CellPreparation {
    source: String,
}

impl CellPreparation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, delta: &str) -> Result<(), String> {
        let length = self
            .source
            .len()
            .checked_add(delta.len())
            .ok_or("prepared cell length overflow")?;
        if length > SOURCE_BYTE_LIMIT {
            return Err(format!("prepared cell exceeds {SOURCE_BYTE_LIMIT} bytes"));
        }
        self.source.push_str(delta);
        Ok(())
    }

    pub fn finish(self) -> Result<PreparedCell, String> {
        if self.source.is_empty() {
            return Err("prepared cell must not be empty".into());
        }
        let digest = blake3::hash(self.source.as_bytes()).to_hex().to_string();
        Ok(PreparedCell {
            source: self.source,
            digest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_deltas_preserve_exact_source() {
        let mut preparation = CellPreparation::new();
        preparation.feed("const x = ").unwrap();
        preparation.feed("1; return x;").unwrap();
        let prepared = preparation.finish().unwrap();
        assert_eq!(prepared.source(), "const x = 1; return x;");
        assert_eq!(
            prepared.digest(),
            blake3::hash(prepared.source().as_bytes()).to_hex().as_str()
        );
    }
}
