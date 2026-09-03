use super::RngSeedGenerator;

impl RngSeedGenerator {
    pub(crate) fn telekio_parts(&self) -> (u32, u32) {
        self.state.lock().unwrap().telekio_parts()
    }
}
