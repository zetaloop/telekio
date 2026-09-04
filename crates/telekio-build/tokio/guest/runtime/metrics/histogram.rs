use super::{HistogramBuilder, HistogramScale, HistogramType, LegacyLogHistogram, LinearHistogram};

impl HistogramBuilder {
    pub(crate) fn telekio_parts(&self) -> (u8, u64, u64, u64) {
        let histogram = match &self.legacy {
            Some(legacy) => match legacy.scale {
                HistogramScale::Linear => HistogramType::Linear(LinearHistogram {
                    num_buckets: legacy.num_buckets,
                    bucket_width: legacy.resolution,
                }),
                HistogramScale::Log => HistogramType::LogLegacy(LegacyLogHistogram {
                    num_buckets: legacy.num_buckets,
                    first_bucket_width: legacy.resolution.next_power_of_two(),
                }),
            },
            None => self.histogram_type,
        };
        match histogram {
            HistogramType::Linear(histogram) => {
                (1, histogram.bucket_width, histogram.num_buckets as u64, 0)
            }
            HistogramType::LogLegacy(histogram) => (
                2,
                histogram.first_bucket_width,
                histogram.num_buckets as u64,
                0,
            ),
            HistogramType::H2(histogram) => (
                3,
                histogram.num_buckets as u64,
                histogram.p as u64,
                histogram.bucket_offset as u64,
            ),
        }
    }
}
