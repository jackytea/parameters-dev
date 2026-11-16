#[must_use]
pub fn closest<'a>(target: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (target.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .filter(|candidate| *candidate != target)
        .filter_map(|candidate| {
            bounded_distance(target, candidate, limit).map(|distance| (distance, candidate))
        })
        .min()
        .map(|(_, candidate)| candidate)
}

fn bounded_distance(left: &str, right: &str, limit: usize) -> Option<usize> {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }
    let mut two_back = vec![0; right.len() + 1];
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for row in 0..left.len() {
        current[0] = row + 1;
        for column in 0..right.len() {
            let substitution = previous[column] + usize::from(left[row] != right[column]);
            let mut distance = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
            if row > 0
                && column > 0
                && left[row] == right[column - 1]
                && left[row - 1] == right[column]
            {
                distance = distance.min(two_back[column - 1] + 1);
            }
            current[column + 1] = distance;
        }
        if current
            .iter()
            .min()
            .is_some_and(|&smallest| smallest > limit)
        {
            return None;
        }
        std::mem::swap(&mut two_back, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    let distance = previous[right.len()];
    (distance <= limit).then_some(distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_close_matches() {
        assert_eq!(
            closest("chekout", ["checkout", "build", "deploy"]),
            Some("checkout")
        );
        assert_eq!(closest("biuld", ["checkout", "build"]), Some("build"));
        assert_eq!(closest("pth", ["path", "status"]), Some("path"));
    }

    #[test]
    fn ignores_distant_and_identical_candidates() {
        assert_eq!(closest("deploy", ["checkout", "build"]), None);
        assert_eq!(closest("build", ["build"]), None);
    }
}
