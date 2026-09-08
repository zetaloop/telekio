use super::Budget;

#[cfg(all(tokio_unstable, feature = "rt"))]
pub(super) fn forced_yield<F>(local: F) -> impl FnOnce(&crate::runtime::scheduler::Handle)
where
    F: FnOnce(&crate::runtime::scheduler::Handle),
{
    move |handle| {
        let state = ::telekio_abi::execution_state();
        if state.is_null() {
            local(handle);
        } else {
            unsafe { (*state).forced_yields += 1 };
        }
    }
}

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
