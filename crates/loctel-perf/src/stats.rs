//! The two summaries the scenarios report: the median and a percentile.

/// The median of `values` (the mean of the two middle ones when there is an
/// even number); sorts them. 0 for none.
pub fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    }
}

/// The `p`th percentile of `values` by the nearest rank (the smallest value
/// that at least `p` percent of them are at or below); sorts them. 0 for none.
pub fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * values.len() as f64).ceil().max(1.0) as usize;
    values[rank.min(values.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_and_percentiles() {
        assert_eq!(median(&mut [5.0, 1.0, 3.0, 2.0, 4.0]), 3.0);
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
        let mut hundred: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&mut hundred, 95.0), 95.0, "nearest rank");
        assert_eq!(percentile(&mut hundred, 99.0), 99.0);
        assert_eq!(percentile(&mut hundred, 100.0), 100.0);
        assert_eq!(percentile(&mut [7.0], 99.0), 7.0);
    }
}
