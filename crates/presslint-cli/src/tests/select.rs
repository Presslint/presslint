use std::fs;

use presslint_selectors::{CompareOp, Predicate, Selector};
use presslint_types::{ColorSpace, ColorUsage};

use crate::{
    args::{Cli, parse_device_link_arg, parse_selector_arg},
    error::CliError,
};

#[test]
fn parses_selector_json() {
    let selector = parse_selector_arg(Some(
        r#"{"op":"predicate","predicate":{"kind":"color_usage","usage":"fill"}}"#,
    ))
    .unwrap()
    .unwrap();

    assert_eq!(
        selector,
        Selector::Predicate {
            predicate: Predicate::ColorUsage {
                usage: ColorUsage::Fill,
            },
        }
    );
}

#[test]
fn parses_component_compare_selector_json() {
    let selector = parse_selector_arg(Some(
        r#"{"op":"predicate","predicate":{"kind":"component_compare","space":"device_cmyk","usage":"fill","component_index":3,"op":"ge","value":0.85}}"#,
    ))
    .unwrap()
    .unwrap();

    assert_eq!(
        selector,
        Selector::Predicate {
            predicate: Predicate::ComponentCompare {
                space: ColorSpace::DeviceCmyk,
                usage: Some(ColorUsage::Fill),
                component_index: 3,
                op: CompareOp::Ge,
                value: 0.85,
            },
        }
    );
}

#[test]
fn parses_selector_from_file() {
    let path = std::env::temp_dir().join(format!(
        "presslint-cli-selector-{}.json",
        std::process::id()
    ));
    fs::write(
        &path,
        r#"{"op":"predicate","predicate":{"kind":"color_space","space":"device_rgb"}}"#,
    )
    .unwrap();

    let arg = format!("@{}", path.display());
    let selector = parse_selector_arg(Some(&arg)).unwrap().unwrap();
    assert_eq!(
        selector,
        Selector::Predicate {
            predicate: Predicate::ColorSpace {
                space: ColorSpace::DeviceRgb,
            },
        }
    );

    let _ = fs::remove_file(path);
}

#[test]
fn selector_json_error_preserves_location() {
    let error = parse_selector_arg(Some("{\n  bad"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("line 2"));
    assert!(error.contains("column"));
}

#[test]
fn rejects_empty_selector_file() {
    let path = std::env::temp_dir().join(format!(
        "presslint-cli-empty-selector-{}.json",
        std::process::id()
    ));
    fs::write(&path, b"").unwrap();

    let arg = format!("@{}", path.display());
    let error = parse_selector_arg(Some(&arg)).unwrap_err().to_string();
    assert!(error.contains("is empty"));

    let _ = fs::remove_file(path);
}

#[test]
fn parses_device_link_id_and_derived_basename() {
    let explicit = parse_device_link_arg("rgb=/tmp/private/path/link.icc").unwrap();
    assert_eq!(explicit.id.as_deref(), Some("rgb"));
    assert_eq!(
        explicit.path.to_string_lossy(),
        "/tmp/private/path/link.icc"
    );

    let derived = parse_device_link_arg("/tmp/private/path/press.icc").unwrap();
    assert_eq!(derived.id.as_deref(), Some("press"));
}

#[test]
fn unsupported_selector_error_names_supported_subset() {
    let error = CliError::Convert(
        presslint_write::ConvertContentColorsError::UnsupportedTargetSelector {
            unsupported: Vec::new(),
        },
    )
    .to_string();
    assert!(error.contains("unsupported target selector leaves"));
    assert!(error.contains("supported selector subset"));
    assert!(error.contains("ComponentCompare"));
    assert!(error.contains("None/Fill/Stroke"));
}

#[test]
fn select_help_names_component_compare_supported_subset() {
    let help = Cli::try_parse_from(["presslint", "convert", "--help"])
        .unwrap_err()
        .to_string();

    assert!(help.contains("ComponentCompare"));
    assert!(help.contains("None/Fill/Stroke"));
}
