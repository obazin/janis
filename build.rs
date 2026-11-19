//! Compiles the selected user interface and generates its translated-strings global.
//!
//! The base catalogs supply shared labels; the `youtube-music` catalogs add the
//! optional provider's labels and shared empty-state wording. Every selected key
//! becomes a Slint property and a Rust setter, so missing interface keys fail to
//! compile and switching language updates every label on screen.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[path = "src/string_names.rs"]
mod string_names;

use string_names::property_name;

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_YOUTUBE_MUSIC");
    println!("cargo:rerun-if-changed=translations");
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=src/string_names.rs");
    let youtube_music = std::env::var_os("CARGO_FEATURE_YOUTUBE_MUSIC").is_some();
    let output_directory =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR for build scripts"));
    let manifest_directory = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );

    let catalog = selected_catalog("en", youtube_music);
    let french_catalog = selected_catalog("fr", youtube_music);
    for (language, selected) in [("en", &catalog), ("fr", &french_catalog)] {
        std::fs::write(
            output_directory.join(format!("{language}.json")),
            serde_json::to_vec(selected).expect("a flat string catalog serializes"),
        )
        .expect("write the selected catalog");
    }

    let mut global = String::from("export global Strings {\n");
    let mut setters = String::from(
        "pub fn apply_strings(strings: &Strings<'_>, text: &dyn Fn(&str) -> String) {\n",
    );
    let mut names = BTreeMap::new();
    for key in catalog.keys() {
        let name = property_name(key);
        if let Some(other) = names.insert(name.clone(), key.clone()) {
            panic!("translation keys `{other}` and `{key}` both map to `{name}`");
        }
        writeln!(global, "    in property <string> {name};").expect("writing to a string");
        writeln!(
            setters,
            "    strings.set_{}(text({key:?}).into());",
            name.replace('-', "_")
        )
        .expect("writing to a string");
    }
    global.push_str("}\n");
    setters.push_str("}\n");
    std::fs::write(output_directory.join("strings.slint"), global).expect("write strings.slint");
    std::fs::write(output_directory.join("strings.rs"), setters).expect("write strings.rs");

    // Slint 1.18 has no source-level feature conditionals, and slint-build does
    // not expose the compiler's import callback. Stage only markup, selecting
    // explicitly delimited provider sections; the original include directory
    // resolves fonts and images without copying or rewriting any resource.
    let interface_directory = output_directory.join("ui");
    if interface_directory.exists() {
        std::fs::remove_dir_all(&interface_directory)
            .expect("remove the previous generated markup");
    }
    stage_interface(
        &manifest_directory.join("ui"),
        Path::new(""),
        &interface_directory,
        youtube_music,
    );
    let configuration = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles)
        .with_include_paths(vec![output_directory, manifest_directory.join("ui")]);
    slint_build::compile_with_config(interface_directory.join("app.slint"), configuration)
        .expect("the user interface compiles");
}

fn selected_catalog(language: &str, youtube_music: bool) -> BTreeMap<String, String> {
    let read_catalog = |path: PathBuf| {
        let source = std::fs::read_to_string(&path).expect("a translation catalog exists");
        serde_json::from_str::<BTreeMap<String, String>>(&source)
            .expect("a translation catalog is a flat JSON object")
    };
    let mut catalog = read_catalog(Path::new("translations").join(format!("{language}.json")));
    if youtube_music {
        let overlay =
            read_catalog(Path::new("translations/youtube-music").join(format!("{language}.json")));
        for (key, value) in overlay {
            let shared_wording = matches!(
                key.as_str(),
                "library.favourites.empty.desc" | "library.playlists.empty.desc"
            );
            assert!(
                key.starts_with("youtubeMusic.") || shared_wording,
                "unexpected provider overlay key: {key}"
            );
            assert_eq!(
                catalog.contains_key(&key),
                shared_wording,
                "only shared empty-state wording overrides a base key: {key}"
            );
            catalog.insert(key, value);
        }
    }
    catalog
}

fn stage_interface(source: &Path, relative: &Path, destination: &Path, youtube_music: bool) {
    std::fs::create_dir_all(destination.join(relative))
        .expect("create the generated markup directory");
    for entry in std::fs::read_dir(source.join(relative)).expect("read an interface directory") {
        let entry = entry.expect("an interface directory entry");
        let relative = relative.join(entry.file_name());
        if !youtube_music
            && (relative == Path::new("features/youtube_music")
                || relative == Path::new("screens/youtube_music_screen.slint"))
        {
            continue;
        }
        if entry.file_type().expect("an interface entry type").is_dir() {
            stage_interface(source, &relative, destination, youtube_music);
        } else if relative
            .extension()
            .is_some_and(|extension| extension == "slint")
        {
            let markup =
                std::fs::read_to_string(source.join(&relative)).expect("read interface markup");
            let expected_sections = if relative == Path::new("app.slint") {
                4
            } else if relative == Path::new("screens/settings_screen.slint") {
                2
            } else if relative == Path::new("features/library/covers.slint") {
                1
            } else {
                0
            };
            let mut selected = String::with_capacity(markup.len());
            let mut inside_section = false;
            let mut sections = 0;
            for line in markup.lines() {
                match line.trim() {
                    "// janis:youtube-music:begin" => {
                        assert!(!inside_section, "nested feature section in {relative:?}");
                        inside_section = true;
                        sections += 1;
                    }
                    "// janis:youtube-music:end" => {
                        assert!(
                            inside_section,
                            "unmatched feature section end in {relative:?}"
                        );
                        inside_section = false;
                    }
                    marker if marker.starts_with("// janis:") => {
                        panic!("unknown feature marker in {relative:?}: {marker}");
                    }
                    _ if !inside_section || youtube_music => selected.push_str(line),
                    _ => {}
                }
                // Preserve source line numbers for compiler diagnostics.
                selected.push('\n');
            }
            assert!(
                !inside_section,
                "unterminated feature section in {relative:?}"
            );
            assert_eq!(
                sections, expected_sections,
                "feature boundaries in {relative:?}"
            );
            std::fs::write(destination.join(relative), selected)
                .expect("write selected interface markup");
        }
    }
}
