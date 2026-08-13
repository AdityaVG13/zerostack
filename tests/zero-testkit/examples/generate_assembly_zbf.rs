use std::{fs, path::PathBuf};

use zero_abi::sha256_hex;
use zero_store::DurableProfileV1;
use zero_testkit::assembly_kat::{
    assembly_manifest_kat_v1, canonical_index_bytes_v1, vector_index_v1, RunnerSourceV1,
    KAT_FIXTURE_RELATIVE_DIR,
};
use zero_testkit::assembly_kat::{zbf_container_kat_v1, zbf_leaf_kat_v1};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(KAT_FIXTURE_RELATIVE_DIR);
    let profile = DurableProfileV1::portable_strict();
    let manifest = assembly_manifest_kat_v1().canonical_bytes().unwrap();
    let leaf = zbf_leaf_kat_v1().to_bytes(profile).unwrap();
    let container = zbf_container_kat_v1().to_bytes(profile).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("assembly-manifest-v1.json"), &manifest).unwrap();
    fs::write(root.join("zbf-leaf-v1.bin"), &leaf).unwrap();
    fs::write(root.join("zbf-container-v1.bin"), &container).unwrap();

    let runner_specs = [
        ("c", 1, "runners/c/verify_v1.c"),
        ("python", 0, "runners/python/verify_v0.py"),
        ("python", 1, "runners/python/verify_v1.py"),
    ];
    let runners = runner_specs
        .into_iter()
        .map(|(language, verifier_version, file)| {
            let bytes = fs::read(root.join(file)).unwrap();
            RunnerSourceV1 {
                language: language.into(),
                verifier_version,
                file: file.into(),
                sha256: sha256_hex(&bytes),
            }
        })
        .collect();
    let index = vector_index_v1(runners);
    fs::write(root.join("index.json"), canonical_index_bytes_v1(&index)).unwrap();
    println!("{}", root.display());
}
