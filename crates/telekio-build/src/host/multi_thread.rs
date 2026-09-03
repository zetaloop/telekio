pub(crate) fn record_poll(actual: u64, guest: u64) {
    super::stats::telekio::record_poll(actual, guest);
}
