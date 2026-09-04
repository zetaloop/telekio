use super::{HistogramBuilder, HistogramType, LegacyLogHistogram, LinearHistogram};

impl HistogramBuilder {
    pub(crate) fn telekio_from_parts(kind: u8, a: u64, b: u64, c: u64) -> Option<Self> {
        let histogram_type = match kind {
            0 => return None,
            1 => HistogramType::Linear(LinearHistogram {
                bucket_width: a,
                num_buckets: b as usize,
            }),
            2 => HistogramType::LogLegacy(LegacyLogHistogram {
                first_bucket_width: a,
                num_buckets: b as usize,
            }),
            3 => HistogramType::H2(super::LogHistogram {
                num_buckets: a as usize,
                p: b as u32,
                bucket_offset: c as usize,
            }),
            _ => panic!("invalid Tokio histogram kind"),
        };
        Some(Self {
            histogram_type,
            legacy: None,
        })
    }
}
