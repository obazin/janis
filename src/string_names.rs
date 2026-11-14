//! How a translation key is spelled as a property of the Slint `Strings`
//! global. `build.rs` includes this file to generate the global, and the
//! catalog tests to find where the interface names a key.

/// `now.trackOf` → `now-track-of`: dots and underscores become dashes, and an
/// uppercase letter becomes a dash and its lowercase, but never at the start,
/// where a dash would not make a Slint identifier.
pub fn property_name(key: &str) -> String {
    let mut name = String::with_capacity(key.len() + 4);
    for character in key.chars() {
        match character {
            '.' | '_' => name.push('-'),
            uppercase if uppercase.is_ascii_uppercase() => {
                if !name.is_empty() {
                    name.push('-');
                }
                name.push(uppercase.to_ascii_lowercase());
            }
            other => name.push(other),
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_translation_key_maps_to_its_slint_property_name() {
        assert_eq!(property_name("now.trackOf"), "now-track-of");
        assert_eq!(property_name("library.tracks.one"), "library-tracks-one");
        assert_eq!(property_name("local.scan_report"), "local-scan-report");
        assert_eq!(property_name("Settings.title"), "settings-title");
    }
}
