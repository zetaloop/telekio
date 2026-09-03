use super::FastRand;
#[cfg(any(feature = "macros", all(feature = "sync", feature = "rt")))]
use super::RngSeed;

impl FastRand {
    #[cfg(any(feature = "macros", all(feature = "sync", feature = "rt")))]
    pub(crate) fn from_telekio(one: u32, two: u32) -> Self {
        Self::from_seed(RngSeed::from_pair(one, two))
    }

    pub(crate) fn telekio_parts(self) -> (u32, u32) {
        (self.one, self.two)
    }
}
