#[derive(Clone, Debug, Default, PartialEq)]
pub struct GainEnvelope {
    regions: Vec<GainRegion>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GainRegion {
    start: f32,
    end: f32,
    gain_db: f32,
}

impl GainEnvelope {
    pub fn gain_at(&self, position: f32) -> f32 {
        self.regions
            .iter()
            .find(|region| position >= region.start && position < region.end)
            .map_or(0.0, |region| region.gain_db)
    }

    pub fn set_region(&mut self, start: f32, end: f32, gain_db: f32) {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let start = start.clamp(0.0, 1.0);
        let end = end.clamp(0.0, 1.0);
        if end <= start {
            return;
        }

        let mut next = Vec::with_capacity(self.regions.len() + 2);
        for region in self.regions.drain(..) {
            if region.end <= start || region.start >= end {
                next.push(region);
                continue;
            }
            if region.start < start {
                next.push(GainRegion {
                    end: start,
                    ..region
                });
            }
            if region.end > end {
                next.push(GainRegion {
                    start: end,
                    ..region
                });
            }
        }
        if gain_db.abs() > f32::EPSILON {
            next.push(GainRegion {
                start,
                end,
                gain_db,
            });
        }
        next.sort_by(|left, right| left.start.total_cmp(&right.start));
        self.regions = merge_adjacent(next);
    }
}

fn merge_adjacent(regions: Vec<GainRegion>) -> Vec<GainRegion> {
    let mut merged: Vec<GainRegion> = Vec::with_capacity(regions.len());
    for region in regions {
        if let Some(previous) = merged.last_mut() {
            if (previous.end - region.start).abs() <= f32::EPSILON
                && (previous.gain_db - region.gain_db).abs() <= f32::EPSILON
            {
                previous.end = region.end;
                continue;
            }
        }
        merged.push(region);
    }
    merged
}

pub fn db_to_linear(gain_db: f32) -> f32 {
    10.0_f32.powf(gain_db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_only_applies_within_selected_region_and_splits_existing_region() {
        let mut envelope = GainEnvelope::default();
        envelope.set_region(0.2, 0.8, 6.0);
        envelope.set_region(0.4, 0.6, -6.0);

        assert_eq!(envelope.gain_at(0.1), 0.0);
        assert_eq!(envelope.gain_at(0.3), 6.0);
        assert_eq!(envelope.gain_at(0.5), -6.0);
        assert_eq!(envelope.gain_at(0.7), 6.0);
        assert_eq!(envelope.gain_at(0.8), 0.0);
    }

    #[test]
    fn setting_zero_gain_removes_only_the_selected_range() {
        let mut envelope = GainEnvelope::default();
        envelope.set_region(0.2, 0.8, 6.0);
        envelope.set_region(0.4, 0.6, 0.0);

        assert_eq!(envelope.gain_at(0.3), 6.0);
        assert_eq!(envelope.gain_at(0.5), 0.0);
        assert_eq!(envelope.gain_at(0.7), 6.0);
    }

    #[test]
    fn decibels_convert_to_linear_amplitude() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(6.0206) - 2.0).abs() < 0.001);
        assert!((db_to_linear(-6.0206) - 0.5).abs() < 0.001);
    }
}
