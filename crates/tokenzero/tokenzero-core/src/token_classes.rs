//! Dimensional token accounting: a count carries its accounting class in the type.
//!
//! Token totals in different classes must never be combined implicitly -- adding
//! visible tokens to raw tokens, or billed-input to cached, is the bug class behind
//! per-event/percentage methodology errors. `Tok<C>` makes every such mix a compile
//! error; the only cross-class path is the audited [`Tok::cast`] escape hatch.

use core::fmt;
use core::iter::Sum;
use core::marker::PhantomData;
use core::ops::{Add, AddAssign, Sub, SubAssign};

mod sealed {
    pub trait Sealed {}
}

/// Accounting class marker. Sealed: the class set is part of the accounting
/// contract, and downstream crates must not mint classes that bypass it.
pub trait TokenClass:
    sealed::Sealed + Copy + Clone + Eq + Ord + core::hash::Hash + Default + 'static
{
    const NAME: &'static str;
}

macro_rules! token_classes {
    ($($(#[$doc:meta])* $name:ident => $label:literal),+ $(,)?) => {
        $(
            $(#[$doc])*
            #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
            pub struct $name;
            impl sealed::Sealed for $name {}
            impl TokenClass for $name {
                const NAME: &'static str = $label;
            }
        )+
    };
}

token_classes! {
    /// Tokens rendered into the model-facing transcript.
    Visible => "visible",
    /// Tokens of the underlying raw content before compaction.
    Raw => "raw",
    /// Input tokens billed at the full (uncached) rate.
    BilledIn => "billed_in",
    /// Output tokens billed by the provider.
    BilledOut => "billed_out",
    /// Input tokens served from a provider cache at the cached rate.
    Cached => "cached",
}

/// A token count in accounting class `C`.
///
/// Arithmetic is class-preserving; mixing classes fails to compile:
///
/// ```compile_fail
/// use tokenzero_core::token_classes::{Raw, Tok, Visible};
/// let visible = Tok::<Visible>::new(10);
/// let raw = Tok::<Raw>::new(20);
/// let _ = visible + raw;
/// ```
///
/// ```compile_fail
/// use tokenzero_core::token_classes::{BilledIn, Cached, Tok};
/// fn billed_total(n: Tok<BilledIn>) -> u64 { n.get() }
/// let cached = Tok::<Cached>::new(5);
/// billed_total(cached);
/// ```
///
/// Same-class arithmetic works as expected:
///
/// ```
/// use tokenzero_core::token_classes::{Tok, Visible};
/// let total: Tok<Visible> = Tok::new(30) + Tok::new(12);
/// assert_eq!(total.get(), 42);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Tok<C: TokenClass> {
    count: u64,
    class: PhantomData<fn() -> C>,
}

impl<C: TokenClass> Tok<C> {
    pub const ZERO: Self = Self::new(0);

    #[must_use]
    pub const fn new(count: u64) -> Self {
        Self {
            count,
            class: PhantomData,
        }
    }

    /// Lossless on every supported target: counts originate as `usize` in the
    /// render/measurement paths and `usize` never exceeds `u64` here.
    #[must_use]
    pub const fn from_usize(count: usize) -> Self {
        Self::new(count as u64)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.count
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.count == 0
    }

    #[must_use]
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.count.checked_add(rhs.count) {
            Some(count) => Some(Self::new(count)),
            None => None,
        }
    }

    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self::new(self.count.saturating_add(rhs.count))
    }

    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self::new(self.count.saturating_sub(rhs.count))
    }

    /// The one legal cross-class conversion. Every call site is an audit point:
    /// reclassification changes what a number *means*, so it must be visible in
    /// review rather than smuggled through arithmetic.
    #[must_use]
    pub const fn cast<D: TokenClass>(self) -> Tok<D> {
        Tok::new(self.count)
    }
}

impl<C: TokenClass> fmt::Debug for Tok<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tok<{}>({})", C::NAME, self.count)
    }
}

impl<C: TokenClass> fmt::Display for Tok<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.count, C::NAME)
    }
}

impl<C: TokenClass> Add for Tok<C> {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        // Saturate so debug panic and release wrap cannot diverge.
        // Callers that need explicit overflow failure use `checked_add`.
        self.saturating_add(rhs)
    }
}

impl<C: TokenClass> AddAssign for Tok<C> {
    fn add_assign(&mut self, rhs: Self) {
        *self = self.saturating_add(rhs);
    }
}

impl<C: TokenClass> Sub for Tok<C> {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        self.saturating_sub(rhs)
    }
}

impl<C: TokenClass> SubAssign for Tok<C> {
    fn sub_assign(&mut self, rhs: Self) {
        *self = self.saturating_sub(rhs);
    }
}

impl<C: TokenClass> Sum for Tok<C> {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

impl<'a, C: TokenClass> Sum<&'a Tok<C>> for Tok<C> {
    fn sum<I: Iterator<Item = &'a Tok<C>>>(iter: I) -> Self {
        iter.copied().sum()
    }
}

/// Class-typed front door to [`crate::tokens::savings_ratio`]: the ratio is only
/// meaningful with raw in the numerator's baseline and visible as the spend, and
/// the types now enforce which argument is which.
#[must_use]
pub fn savings_ratio_typed(raw: Tok<Raw>, visible: Tok<Visible>) -> f64 {
    crate::tokens::savings_ratio_u64(raw.get(), visible.get())
}

