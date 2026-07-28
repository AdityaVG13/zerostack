use std::borrow::Cow;
use zero_cert::*;

pub struct Resident<'a> {
    pub bytes: &'a [u8],
    pub operator: Option<&'a str>,
    pub parser: Option<&'a str>,
    pub index: Option<&'a str>,
}
impl Resolver for Resident<'_> {
    fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> { (zero_abi::sha256(self.bytes) == object_id.0).then_some(self.bytes) }
    fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> { (id == "read-span").then_some(self.operator).flatten() }
    fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> { (id == "tree-sitter").then_some(self.parser).flatten() }
    fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> { (id == "zero-index").then_some(self.index).flatten() }
}
pub fn fixture(bytes: &[u8]) -> (EvidenceCertificate<'_>, Resident<'_>) {
    let digest = zero_abi::sha256(bytes);
    let span = SpanRef { object_id: ObjectId(digest), byte_start: 0, byte_len: bytes.len() as u64, object_digest: digest, span_digest: digest };
    let certificate = EvidenceCertificate {
        query: Query::ReadSpan(span.clone()), spans: vec![span], payload: Cow::Borrowed(bytes),
        provenance: Provenance { parser_id: "tree-sitter".into(), parser_version: "1".into(), index_id: "zero-index".into(), index_version: "2".into(), operator_id: "read-span".into(), operator_version: "1".into() },
        completeness: CompletenessWitness::ReadSpan { operator: OperatorLock { operator_id: "read-span".into(), operator_version: "1".into() } },
        input_token_cost: 3, backend_work_units: 1,
    };
    (certificate, Resident { bytes, operator: Some("1"), parser: Some("1"), index: Some("2") })
}
