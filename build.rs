//! Compiles the user interface and generates the translated-strings global.
//!
//! `translations/en.json` is the reference catalog: every key in it becomes a
//! property of the Slint `Strings` global, so a component that names a missing
//! key fails to compile, and a Rust function that fills every property from a
//! lookup, so switching language updates every label on screen.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

#[path = "src/string_names.rs"]
mod string_names;

use string_names::property_name;

const REFERENCE_CATALOG: &str = "translations/en.json";

fn main() {
    println!("cargo:rerun-if-changed={REFERENCE_CATALOG}");
    println!("cargo:rerun-if-changed=src/string_names.rs");
    let output_directory =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR for build scripts"));

    let catalog = std::fs::read_to_string(REFERENCE_CATALOG).expect("the reference catalog exists");
    let catalog: BTreeMap<String, String> =
        serde_json::from_str(&catalog).expect("the reference catalog is a flat JSON object");

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

    let configuration = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles)
        .with_include_paths(vec![output_directory]);
    slint_build::compile_with_config("ui/app.slint", configuration)
        .expect("the user interface compiles");
}
