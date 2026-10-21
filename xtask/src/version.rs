//! Release versions: which `vX.Y.Z` tags exist, and which one comes next.
//!
//! Every merge to main is a release with a patch bump (design 10.3), so the
//! next version is the highest `vX.Y.Z` tag with its patch plus one. A person
//! who wants a bigger bump pushes a tag by hand (say `v0.2.0`); the next merge
//! then continues from it.

/// The version a tag names, as (major, minor, patch). `None` for anything that
/// is not exactly `vX.Y.Z` with plain decimal numbers: no `v`, a missing or
/// extra part, a pre-release suffix, a sign, a leading zero, or a part too
/// big for a `u32`. Such tags belong to somebody else and are left alone.
fn parse_tag(tag: &str) -> Option<(u32, u32, u32)> {
    fn part(text: &str) -> Option<u32> {
        let plain = !text.is_empty()
            && text.bytes().all(|byte| byte.is_ascii_digit())
            && (text == "0" || !text.starts_with('0'));
        if plain { text.parse().ok() } else { None }
    }

    let mut parts = tag.strip_prefix('v')?.split('.');
    let version = (
        part(parts.next()?)?,
        part(parts.next()?)?,
        part(parts.next()?)?,
    );
    parts.next().is_none().then_some(version)
}

/// The highest `vX.Y.Z` among `tags` (numerically, so `v0.1.10` beats
/// `v0.1.9`), or `None` when there is none.
pub fn latest_tag(tags: &[String]) -> Option<String> {
    tags.iter()
        .filter_map(|tag| parse_tag(tag).map(|version| (version, tag)))
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag.clone())
}

/// The next release: the highest `vX.Y.Z` in `tags` with its patch bumped, or
/// `v0.1.0` when there is no such tag.
pub fn next_version(tags: &[String]) -> String {
    let Some((major, minor, patch)) = tags.iter().filter_map(|tag| parse_tag(tag)).max() else {
        return "v0.1.0".to_owned();
    };
    // Widened first, so that a patch of `u32::MAX` still has a successor.
    format!("v{major}.{minor}.{}", u64::from(patch) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn next_version_from_no_tags() {
        assert_eq!(next_version(&[]), "v0.1.0");
    }

    #[test]
    fn next_version_bumps_patch_of_highest() {
        // 10 is higher than 4, which a text sort would get wrong.
        assert_eq!(
            next_version(&tags(&["v0.1.4", "v0.1.10", "v0.0.9"])),
            "v0.1.11"
        );
    }

    #[test]
    fn the_order_of_the_tags_does_not_matter() {
        assert_eq!(
            next_version(&tags(&["v0.0.9", "v0.1.10", "v0.1.4"])),
            "v0.1.11"
        );
    }

    #[test]
    fn minor_and_major_outrank_patch() {
        assert_eq!(next_version(&tags(&["v0.9.9", "v1.0.0"])), "v1.0.1");
        assert_eq!(next_version(&tags(&["v1.2.30", "v1.10.0"])), "v1.10.1");
    }

    #[test]
    fn tags_that_are_not_versions_are_ignored() {
        let noise = tags(&[
            "latest",
            "1.2.3",
            "v1.2",
            "v1.2.3.4",
            "v1.2.3-rc1",
            "v1.2.x",
            "v1.2.+3",
            "v01.2.3",
            "v99999999999.0.0",
            "",
        ]);
        assert_eq!(next_version(&noise), "v0.1.0");

        let mut mixed = noise;
        mixed.push("v0.2.5".to_owned());
        assert_eq!(next_version(&mixed), "v0.2.6");
    }

    #[test]
    fn latest_tag_is_the_highest_version_tag() {
        assert_eq!(latest_tag(&[]), None);
        assert_eq!(latest_tag(&tags(&["nightly", "v1.2"])), None);
        assert_eq!(
            latest_tag(&tags(&["v0.1.4", "nightly", "v0.1.10", "v0.0.9"])),
            Some("v0.1.10".to_owned())
        );
    }
}
