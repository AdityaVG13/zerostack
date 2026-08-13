use std::borrow::Cow;
use zero_cert::*;

pub struct Resident<'a> {
    pub bytes: &'a [u8],
    pub operator: Option<&'a str>,
    pub parser: Option<&'a str>,
    pub index: Option<&'a str>,
}
impl Resolver for Resident<'_> {
    fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
        (zero_abi::sha256(self.bytes) == object_id.0).then_some(self.bytes)
    }
    fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "read-span").then_some(self.operator).flatten()
    }
    fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "tree-sitter").then_some(self.parser).flatten()
    }
    fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "zero-index").then_some(self.index).flatten()
    }
}
pub fn fixture(bytes: &[u8]) -> (EvidenceCertificate<'_>, Resident<'_>) {
    let (span, selected) =
        SpanRef::from_fragment(bytes, &zero_ref::ZeroFragment::None, "fixture").unwrap();
    assert_eq!(selected, bytes);
    let certificate = EvidenceCertificate {
        query: Query::ReadSpan(span.clone()),
        spans: vec![span],
        payload: Cow::Borrowed(bytes),
        provenance: Provenance {
            parser_id: "tree-sitter".into(),
            parser_version: "1".into(),
            index_id: "zero-index".into(),
            index_version: "2".into(),
            operator_id: "read-span".into(),
            operator_version: "1".into(),
        },
        completeness: CompletenessWitness::ReadSpan {
            operator: OperatorLock {
                operator_id: "read-span".into(),
                operator_version: "1".into(),
            },
        },
        input_token_cost: 3,
        backend_work_units: 1,
    };
    (
        certificate,
        Resident {
            bytes,
            operator: Some("1"),
            parser: Some("1"),
            index: Some("2"),
        },
    )
}

pub struct Residents<'a> {
    pub objects: Vec<&'a [u8]>,
    pub mutation_receipts: Vec<(Digest, u64, &'a [u8])>,
    pub aggregate_receipts: Vec<(Digest, &'a [u8])>,
}
impl Resolver for Residents<'_> {
    fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
        self.objects
            .iter()
            .copied()
            .find(|bytes| zero_abi::sha256(bytes) == object_id.0)
    }
    fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "read-span").then_some("1")
    }
    fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "tree-sitter").then_some("1")
    }
    fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "zero-index").then_some("2")
    }
    fn resolve_mutation_receipt<'a>(
        &'a self,
        journal_id: &Digest,
        sequence: u64,
    ) -> Option<&'a [u8]> {
        self.mutation_receipts
            .iter()
            .find_map(|(id, seq, bytes)| (id == journal_id && *seq == sequence).then_some(*bytes))
    }
    fn resolve_aggregate_receipt<'a>(&'a self, snapshot_id: &Digest) -> Option<&'a [u8]> {
        self.aggregate_receipts
            .iter()
            .find_map(|(id, bytes)| (id == snapshot_id).then_some(*bytes))
    }
}

pub fn object_id(bytes: &[u8]) -> ObjectId {
    ObjectId(zero_abi::sha256(bytes))
}
pub fn provenance() -> Provenance {
    Provenance {
        parser_id: "tree-sitter".into(),
        parser_version: "1".into(),
        index_id: "zero-index".into(),
        index_version: "2".into(),
        operator_id: "read-span".into(),
        operator_version: "1".into(),
    }
}
pub fn span(bytes: &[u8], start: usize, len: usize) -> SpanRef {
    SpanRef {
        object_id: object_id(bytes),
        object_digest: zero_abi::sha256(bytes),
        byte_start: start as u64,
        byte_len: len as u64,
        span_digest: zero_abi::sha256(&bytes[start..start + len]),
    }
}
