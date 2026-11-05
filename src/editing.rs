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

    pub fn adjust_region(&mut self, start: f32, end: f32, gain_delta_db: f32) {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let start = start.clamp(0.0, 1.0);
        let end = end.clamp(0.0, 1.0);
        if end <= start || !gain_delta_db.is_finite() {
            return;
        }

        let mut boundaries = vec![start, end];
        for region in &self.regions {
            if region.start > start && region.start < end {
                boundaries.push(region.start);
            }
            if region.end > start && region.end < end {
                boundaries.push(region.end);
            }
        }
        boundaries.sort_by(f32::total_cmp);
        boundaries.dedup_by(|left, right| (*left - *right).abs() <= f32::EPSILON);
        let adjusted: Vec<_> = boundaries
            .windows(2)
            .map(|pair| {
                let midpoint = (pair[0] + pair[1]) * 0.5;
                (pair[0], pair[1], self.gain_at(midpoint) + gain_delta_db)
            })
            .collect();

        self.set_region(start, end, 0.0);
        for (segment_start, segment_end, gain_db) in adjusted {
            self.set_region(segment_start, segment_end, gain_db);
        }
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

pub fn normalize_region(
    samples: &[f32],
    channels: usize,
    selection: [f32; 2],
    target_db: f32,
    envelope: &mut GainEnvelope,
) -> Result<f32, &'static str> {
    if channels == 0 || samples.is_empty() || samples.len() % channels != 0 {
        return Err("Audio has no valid samples to normalize");
    }
    if !target_db.is_finite() || target_db > 0.0 {
        return Err("Normalize target must be at or below 0 dBFS");
    }

    let frame_count = samples.len() / channels;
    let start = selection[0].min(selection[1]).clamp(0.0, 1.0);
    let end = selection[0].max(selection[1]).clamp(0.0, 1.0);
    let first_frame = (start * frame_count as f32).floor() as usize;
    let end_frame = ((end * frame_count as f32).ceil() as usize).min(frame_count);
    if end_frame <= first_frame {
        return Err("Select a non-empty audio region to normalize");
    }

    let mut peak = 0.0_f32;
    for frame_index in first_frame..end_frame {
        let position = frame_index as f32 / frame_count as f32;
        let amplitude = db_to_linear(envelope.gain_at(position));
        let sample_offset = frame_index * channels;
        for &sample in &samples[sample_offset..sample_offset + channels] {
            if sample.is_finite() {
                peak = peak.max((sample * amplitude).abs());
            }
        }
    }
    if peak <= f32::MIN_POSITIVE {
        return Err("Selected audio is silent; normalization was not applied");
    }

    let gain_delta_db = target_db - 20.0 * peak.log10();
    if !gain_delta_db.is_finite() {
        return Err("Could not calculate a safe normalization gain");
    }
    envelope.adjust_region(start, end, gain_delta_db);
    Ok(gain_delta_db)
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

    #[test]
    fn normalize_uses_highest_absolute_peak_across_channels_and_preserves_outside_audio() {
        let samples = [0.2, -0.5, 0.4, -0.9, 0.8, -0.1, 0.1, 0.1];
        let mut envelope = GainEnvelope::default();
        envelope.set_region(0.0, 1.0, 3.0);
        let delta = normalize_region(&samples, 2, [0.25, 0.75], -1.0, &mut envelope).unwrap();

        assert!(delta < -3.0);
        assert!((envelope.gain_at(0.5) - (-1.0 - 20.0 * 0.9_f32.log10())).abs() < 0.001);
        let normalized_peak = 0.9 * db_to_linear(envelope.gain_at(0.25));
        assert!((normalized_peak - db_to_linear(-1.0)).abs() < 0.001);
        assert_eq!(envelope.gain_at(0.1), 3.0);
        assert_eq!(envelope.gain_at(0.9), 3.0);
    }

    #[test]
    fn normalize_leaves_silent_audio_unchanged() {
        let samples = [0.0, 0.0, 0.0, 0.0];
        let mut envelope = GainEnvelope::default();

        assert!(normalize_region(&samples, 2, [0.0, 1.0], -1.0, &mut envelope).is_err());
        assert_eq!(envelope, GainEnvelope::default());
    }
}
