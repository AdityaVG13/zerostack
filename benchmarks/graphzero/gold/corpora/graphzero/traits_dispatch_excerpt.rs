// Excerpt mirroring GraphZero's Expander trait dispatch
// (crates/graphzero-store/src/store/expand.rs): default trait methods and
// generic bounds route calls through types a name-matching pass cannot see.

pub trait Expander {
    fn seed(&self) -> u32;

    fn expand(&self, depth: u32) -> u32 {
        self.seed() + depth
    }
}

pub struct BfsExpander {
    root: u32,
}

impl BfsExpander {
    pub fn new(root: u32) -> Self {
        BfsExpander { root }
    }
}

impl Expander for BfsExpander {
    fn seed(&self) -> u32 {
        self.root
    }
}

pub struct NoopExpander;

pub trait Sealed {
    fn seal(&self);
}

pub fn blast<E: Expander>(expander: &E, depth: u32) -> u32 {
    expander.expand(depth)
}

pub fn blast_default(depth: u32) -> u32 {
    blast(&BfsExpander::new(1), depth)
}

fn unreachable_seed() -> u32 {
    7
}
