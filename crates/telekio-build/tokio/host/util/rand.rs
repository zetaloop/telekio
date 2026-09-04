use super::{FastRand, RngSeed};

impl RngSeed {
    pub(crate) fn from_telekio(one: u32, two: u32) -> Self {
        Self::from_pair(one, two)
    }
}

impl FastRand {
    pub(crate) fn from_telekio(one: u32, two: u32) -> Self {
        Self::from_seed(RngSeed::from_telekio(one, two))
    }

    pub(crate) fn telekio_parts(self) -> (u32, u32) {
        (self.one, self.two)
    }
}
