use std::{fs, path::PathBuf};

use presslint_types::PageIndex;
use presslint_write::PageSelection;

use crate::{
    args::{Cli, clap_error_exit_code, parse_pages, same_input_output},
    convert::validate_output_routing,
};

#[test]
fn parses_all_pages() {
    assert_eq!(parse_pages("all").unwrap(), PageSelection::All);
    assert_eq!(parse_pages(" ALL ").unwrap(), PageSelection::All);
}

#[test]
fn parses_single_list_range_and_mixed_pages_as_zero_based_sorted_unique() {
    assert_eq!(
        parse_pages("1").unwrap(),
        PageSelection::Indices(vec![PageIndex(0)])
    );
    assert_eq!(
        parse_pages("1,3,5").unwrap(),
        PageSelection::Indices(vec![PageIndex(0), PageIndex(2), PageIndex(4)])
    );
    assert_eq!(
        parse_pages("2-4").unwrap(),
        PageSelection::Indices(vec![PageIndex(1), PageIndex(2), PageIndex(3)])
    );
    assert_eq!(
        parse_pages("9,1,3-5,3").unwrap(),
        PageSelection::Indices(vec![
            PageIndex(0),
            PageIndex(2),
            PageIndex(3),
            PageIndex(4),
            PageIndex(8),
        ])
    );
}

#[test]
fn rejects_bad_page_specs() {
    for spec in ["", "0", "1,,2", "3-2", "abc", "1-", "-3"] {
        assert!(parse_pages(spec).is_err(), "{spec}");
    }
}

#[test]
fn rejects_absurd_page_range_before_expansion() {
    let error = parse_pages("1-1000001").unwrap_err().to_string();
    assert!(error.contains("maximum explicit selection is 1000000"));
}

#[test]
fn rejects_json_report_when_pdf_would_use_stdout() {
    let args = crate::args::ConvertArgs {
        input: PathBuf::from("in.pdf"),
        device_links: vec!["link.icc".to_owned()],
        select: None,
        pages: "all".to_owned(),
        preserve_black: false,
        json: true,
        output: "-".to_owned(),
    };

    let error = validate_output_routing(&args).unwrap_err().to_string();
    assert!(error.contains("--json cannot be combined with -o -"));
}

#[test]
fn detects_equal_input_and_output_paths() {
    let dir = std::env::temp_dir().join(format!("presslint-cli-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("same.pdf");
    fs::write(&path, b"%PDF").unwrap();

    assert!(same_input_output(&path, &path).unwrap());

    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn distinct_missing_bare_relative_output_is_not_equal_to_input() {
    let dir = std::env::temp_dir().join(format!(
        "presslint-cli-bare-output-test-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.pdf");
    fs::write(&input, b"%PDF").unwrap();

    let output = PathBuf::from(format!(
        "presslint-cli-missing-output-{}.pdf",
        std::process::id()
    ));
    let result = same_input_output(&input, &output);

    assert!(!result.unwrap());

    let _ = fs::remove_file(&input);
    let _ = fs::remove_dir(&dir);
}

#[test]
fn clap_usage_errors_exit_one_but_help_stays_successful() {
    let usage_error = Cli::try_parse_from(["presslint", "convert"]).unwrap_err();
    assert_eq!(clap_error_exit_code(&usage_error), 1);

    let help = Cli::try_parse_from(["presslint", "--help"]).unwrap_err();
    assert_eq!(clap_error_exit_code(&help), 0);
}
