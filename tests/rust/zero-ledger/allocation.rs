#![forbid(unsafe_code)]

use allocation_counter::measure;
use zero_ledger::{Digest, LedgerConfig, ResourceGauge, TokenCharge, TokenizerIdentity};

#[test]
fn warmed_successful_charge_allocates_nothing() {
    let tokenizer = TokenizerIdentity::new("cl100k_base", Digest([7; 32]));
    let mut gauge = ResourceGauge::new(LedgerConfig::new(tokenizer.clone()));
    let charge = TokenCharge {
        raw_input_tokens: 1_024,
        input_tokens: 256,
        billed_tokens: 256,
        model_calls: 1,
        ..TokenCharge::default()
    };

    gauge
        .charge(&tokenizer, &charge)
        .expect("warm-up charge uses the locked tokenizer");

    let allocations = measure(|| {
        gauge
            .charge(&tokenizer, &charge)
            .expect("measured charge uses the locked tokenizer");
    });

    assert_eq!(allocations.count_total, 0, "warmed charge allocated");
}
