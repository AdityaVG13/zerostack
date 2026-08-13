use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use zero_testkit::authority::generate_authority;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let authority = root.join("conformance/authority");
    let sources = authority.join("sources");
    let bytes = generate_authority(
        &fs::read(sources.join("round5-claim-ledger.json"))?,
        &fs::read(sources.join("round8-executable-freezes.json"))?,
        &fs::read(sources.join("round5-source-archive-audit.json"))?,
        &fs::read(authority.join("provenance-v1.json"))?,
    )?;
    io::stdout().write_all(&bytes)?;
    Ok(())
}
