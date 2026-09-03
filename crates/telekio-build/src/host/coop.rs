use super::Budget;

impl Budget {
    pub(crate) fn from_telekio(value: u16) -> Self {
        if value == u16::MAX {
            Self::unconstrained()
        } else {
            Self(Some(value.try_into().expect("invalid Tokio task budget")))
        }
    }

    pub(crate) fn telekio_value(self) -> u16 {
        self.0.map_or(u16::MAX, u16::from)
    }
}
