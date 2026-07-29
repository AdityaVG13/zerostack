use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use std::borrow::Cow;
use zero_cert::*;
struct Resident(Vec<u8>);
impl Resolver for Resident {
    fn resolve<'a>(&'a self, id: &ObjectId) -> Option<&'a [u8]> {
        (zero_abi::sha256(&self.0) == id.0).then_some(&self.0)
    }
    fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "read").then_some("1")
    }
    fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "p").then_some("1")
    }
    fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "i").then_some("1")
    }
}
fn resident_verify(c: &mut Criterion) {
    let resident = Resident(vec![0x5a; 64 * 1024]);
    let (span, _) =
        SpanRef::from_fragment(&resident.0, &zero_ref::ZeroFragment::None, "benchmark").unwrap();
    let certificate = EvidenceCertificate {
        query: Query::ReadSpan(span.clone()),
        spans: vec![span],
        payload: Cow::Borrowed(&resident.0),
        provenance: Provenance {
            parser_id: "p".into(),
            parser_version: "1".into(),
            index_id: "i".into(),
            index_version: "1".into(),
            operator_id: "read".into(),
            operator_version: "1".into(),
        },
        completeness: CompletenessWitness::ReadSpan {
            operator: OperatorLock {
                operator_id: "read".into(),
                operator_version: "1".into(),
            },
        },
        input_token_cost: 0,
        backend_work_units: 1,
    };
    let mut group = c.benchmark_group("zero_cert_verify");
    group.throughput(Throughput::Bytes(resident.0.len() as u64));
    group.bench_function("resident_64k", |b| {
        b.iter(|| black_box(verify(black_box(&certificate), black_box(&resident)).unwrap()))
    });
    group.finish();
}
criterion_group!(benches, resident_verify);
criterion_main!(benches);
