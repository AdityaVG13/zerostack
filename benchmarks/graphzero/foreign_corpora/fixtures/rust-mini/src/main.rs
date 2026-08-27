use rust_mini::{parse_config, run_index};

fn main() {
    let cfg = parse_config("demo");
    println!("{}", run_index(&cfg));
}
