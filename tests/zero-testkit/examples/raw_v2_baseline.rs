use zero_testkit::raw_v2_slice::{reference_raw_v2_input_v1, run_raw_v2_slice_v1};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let run = run_raw_v2_slice_v1(&reference_raw_v2_input_v1())?;
    println!("{}", String::from_utf8(run.receipt.canonical_bytes()?)?);
    Ok(())
}
