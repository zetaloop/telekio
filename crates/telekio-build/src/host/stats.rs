use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

thread_local! {
    static POLL_BATCHES: RefCell<Vec<i128>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn start_poll_batch() -> Instant {
    POLL_BATCHES.with(|batches| batches.borrow_mut().push(0));
    Instant::now()
}

pub(crate) fn finish_poll_batch() -> Instant {
    let adjustment = POLL_BATCHES.with(|batches| {
        batches
            .borrow_mut()
            .pop()
            .expect("Tokio poll batch was not started")
    });
    let now = Instant::now();
    if adjustment >= 0 {
        now.checked_sub(Duration::from_nanos(adjustment.min(u64::MAX.into()) as u64))
            .unwrap_or(now)
    } else {
        now.checked_add(Duration::from_nanos(
            (-adjustment).min(u64::MAX.into()) as u64
        ))
        .unwrap_or(now)
    }
}

pub(crate) fn record_poll(actual: u64, guest: u64) {
    POLL_BATCHES.with(|batches| {
        if let Some(adjustment) = batches.borrow_mut().last_mut() {
            *adjustment = adjustment.saturating_add(i128::from(actual) - i128::from(guest));
        }
    });
}
