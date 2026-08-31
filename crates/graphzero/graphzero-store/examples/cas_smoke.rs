//! Manual fresh-root/restart smoke for the canonical shared CAS (ADR 002 §7).
//! Run twice against the same root: the first process publishes, the second
//! (fresh process = "restart") resolves the same object.
fn main() {
    let root = std::env::args()
        .nth(1)
        .expect("usage: cas_smoke <store-root>");
    let cas = graphzero_store::SharedCas::open(&root);
    let hash = cas.put(b"smoke payload: fresh root and restart\n").unwrap();
    println!("put -> {hash} (idempotent)");
    let resolver = graphzero_store::ExpandResolver::new(std::path::Path::new(&root), None).unwrap();
    let hit = resolver
        .resolve_blob(&hash, &format!("z://blob/{hash}"))
        .unwrap();
    println!("resolve -> {} bytes via '{}'", hit.bytes.len(), hit.source);
}
