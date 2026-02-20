use std::collections::BTreeSet;
use std::path::Path;

fn read_root_file(path: &str) -> String {
    let full_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&full_path).unwrap_or_else(|error| {
        panic!(
            "failed to read `{}` from `{}`: {error}",
            path,
            full_path.display()
        );
    })
}

fn parse_manifest() -> toml::Value {
    let manifest = read_root_file("Cargo.toml");
    toml::from_str(&manifest)
        .unwrap_or_else(|error| panic!("failed to parse root Cargo.toml: {error}"))
}

fn features_table(manifest: &toml::Value) -> &toml::value::Table {
    manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("missing `[features]` section in root Cargo.toml"))
}

fn feature_values(manifest: &toml::Value, feature: &str) -> BTreeSet<String> {
    let values = features_table(manifest)
        .get(feature)
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("missing feature list for `{feature}` in root Cargo.toml"));

    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("feature `{feature}` contains a non-string entry"))
                .to_owned()
        })
        .collect()
}

fn optional_feature_names(manifest: &toml::Value) -> BTreeSet<String> {
    manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("missing `[dependencies]` section in root Cargo.toml"))
        .iter()
        .filter_map(|(dependency, value)| {
            if !dependency.starts_with("waterkit-") {
                return None;
            }

            let table = value.as_table().unwrap_or_else(|| {
                panic!("dependency `{dependency}` must be defined with an inline table")
            });

            let is_optional = table
                .get("optional")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);

            if is_optional {
                Some(dependency.trim_start_matches("waterkit-").to_owned())
            } else {
                None
            }
        })
        .collect()
}

fn as_sorted_list(values: &BTreeSet<String>) -> String {
    values.iter().cloned().collect::<Vec<_>>().join(", ")
}

#[test]
fn full_feature_covers_all_optional_feature_crates() {
    let manifest = parse_manifest();
    let optional_features = optional_feature_names(&manifest);
    let full_features = feature_values(&manifest, "full");

    let missing_in_full = optional_features
        .difference(&full_features)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(
        missing_in_full.is_empty(),
        "`full` is missing optional modules: {}",
        as_sorted_list(&missing_in_full)
    );

    let unexpected_in_full = full_features
        .difference(&optional_features)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(
        unexpected_in_full.is_empty(),
        "`full` contains entries without matching optional module dependencies: {}",
        as_sorted_list(&unexpected_in_full)
    );
}

#[test]
fn each_public_feature_binds_its_optional_dependency() {
    let manifest = parse_manifest();
    let optional_features = optional_feature_names(&manifest);

    for feature in &optional_features {
        let expected_dependency = format!("dep:waterkit-{feature}");
        let mappings = feature_values(&manifest, feature);

        assert!(
            mappings.contains(&expected_dependency),
            "feature `{feature}` must include `{expected_dependency}`, got: {}",
            as_sorted_list(&mappings)
        );
    }
}

#[test]
fn lib_rs_reexports_every_public_feature() {
    let manifest = parse_manifest();
    let optional_features = optional_feature_names(&manifest);
    let lib_source = read_root_file("src/lib.rs");

    for feature in &optional_features {
        let cfg_marker = format!("#[cfg(feature = \"{feature}\")]");
        assert!(
            lib_source.contains(&cfg_marker),
            "`src/lib.rs` is missing cfg gate for feature `{feature}`"
        );

        let use_marker = format!("pub use waterkit_{feature} as {feature};");
        assert!(
            lib_source.contains(&use_marker),
            "`src/lib.rs` is missing re-export `{use_marker}`"
        );
    }
}

#[test]
fn readme_lists_every_public_feature_module() {
    let manifest = parse_manifest();
    let optional_features = optional_feature_names(&manifest);
    let readme = read_root_file("README.md");

    for feature in &optional_features {
        let readme_link = format!("]({feature})");
        assert!(
            readme.contains(&readme_link),
            "README is missing module link for feature `{feature}`"
        );
    }
}
