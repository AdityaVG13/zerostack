//! Foreign mini corpus for GraphZero rebaseline (graphzero-e1k1).
//! Not GraphZero product code -- intentional non-self substrate.

pub fn parse_config(input: &str) -> Config {
    Config {
        name: input.trim().to_string(),
        enabled: true,
    }
}

pub struct Config {
    pub name: String,
    pub enabled: bool,
}

pub fn run_index(cfg: &Config) -> usize {
    if cfg.enabled {
        cfg.name.len()
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_and_run() {
        let c = parse_config(" alpha ");
        assert_eq!(run_index(&c), 5);
    }
}
