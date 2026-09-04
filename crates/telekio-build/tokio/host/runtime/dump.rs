use super::*;

impl Dump {
    #[doc(hidden)]
    pub fn telekio_encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        push(&mut bytes, self.tasks.tasks.len());
        for task in &self.tasks.tasks {
            push(&mut bytes, task.id.telekio_value());
            let backtraces = task.trace.inner.backtraces();
            push(&mut bytes, backtraces.len());
            for backtrace in backtraces {
                push(&mut bytes, backtrace.len());
                for frame in backtrace {
                    push(&mut bytes, frame.ip() as usize);
                    push(&mut bytes, frame.symbol_address() as usize);
                }
            }
        }
        bytes
    }
}

fn push(bytes: &mut Vec<u8>, value: impl TryInto<u64>) {
    bytes.extend_from_slice(
        &value
            .try_into()
            .ok()
            .expect("Tokio dump value overflow")
            .to_le_bytes(),
    );
}
