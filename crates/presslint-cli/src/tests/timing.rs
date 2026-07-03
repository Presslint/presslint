#![allow(clippy::unwrap_used)]

use std::time::Duration;

use crate::{
    args::{AuditArgs, ConvertArgs},
    audit, convert,
    report::render_timing,
};

const RGB_TO_CMYK_LINK: &str = "000001746c636d73043000006c696e6b52474220434d594b07ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d730000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014132423000000090000000e46d4142200000000003040000000000a40000000000000000000000500000002070617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000002020200000000000000000000000000020000000005002a004f0074009900be00e30108012d01520177019c01c101e6020b0230025500220047006c009100b600db01000125014a016f019401b901de02030228706172610000000000000000000100007061726100000000000000000001000070617261000000000000000000010000706170617261000000000000000000010000";

#[test]
fn render_timing_preserves_labels_order_and_total() {
    let phases = [
        ("read_input", Duration::from_millis(41)),
        ("convert", Duration::from_millis(12)),
        ("write_output", Duration::from_millis(88)),
    ];
    let mut rendered = Vec::new();

    render_timing(&phases, Duration::from_millis(141), &mut rendered).unwrap();

    assert_eq!(
        String::from_utf8(rendered).unwrap(),
        "timing: read_input 41ms convert 12ms write_output 88ms total 141ms\n"
    );
}

#[test]
fn render_timing_uses_microseconds_below_one_millisecond() {
    let phases = [("audit", Duration::from_micros(750))];
    let mut rendered = Vec::new();

    render_timing(&phases, Duration::from_micros(999), &mut rendered).unwrap();

    assert_eq!(
        String::from_utf8(rendered).unwrap(),
        "timing: audit 750µs total 999µs\n"
    );
}

#[test]
fn audit_json_report_is_identical_with_and_without_timing() {
    let dir = temp_dir("presslint-cli-audit-timing");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.pdf");
    std::fs::write(&input_path, classic_raw_pdf(b"1 0 0 rg\n")).unwrap();

    let untimed_args = AuditArgs {
        input: input_path.clone(),
        json: true,
        timing: false,
    };
    let timed_args = AuditArgs {
        input: input_path.clone(),
        json: true,
        timing: true,
    };

    let untimed_json = audit::run(&untimed_args).unwrap().to_json_string().unwrap();
    let timed_json = audit::run(&timed_args).unwrap().to_json_string().unwrap();

    assert_eq!(untimed_json, timed_json);

    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn convert_timing_with_stdout_output_keeps_stdout_pdf_only() {
    let dir = temp_dir("presslint-cli-convert-timing");
    std::fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.pdf");
    let link_path = dir.join("synthetic-rgb-cmyk.icc");
    let file_output = dir.join("output.pdf");
    let input = classic_raw_pdf(b"1 0 0 rg\n");

    std::fs::write(&input_path, &input).unwrap();
    std::fs::write(&link_path, link_bytes(RGB_TO_CMYK_LINK)).unwrap();

    let file_args = ConvertArgs {
        input: input_path.clone(),
        device_links: vec![format!("rgb={}", link_path.display())],
        select: None,
        pages: "all".to_owned(),
        preserve_black: false,
        json: false,
        timing: false,
        output: file_output.to_string_lossy().into_owned(),
    };
    convert::run(&file_args).unwrap();
    let expected_pdf = std::fs::read(&file_output).unwrap();

    let stdout_args = ConvertArgs {
        output: "-".to_owned(),
        timing: true,
        ..file_args
    };
    let mut stdout = Vec::new();
    convert::run_with_pdf_stdout(&stdout_args, &mut stdout).unwrap();

    assert_eq!(stdout, expected_pdf);
    assert!(!String::from_utf8_lossy(&stdout).contains("timing:"));

    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_file(&link_path);
    let _ = std::fs::remove_file(&file_output);
    let _ = std::fs::remove_dir(dir);
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", std::process::id()))
}

fn link_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn classic_raw_pdf(data: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
        stream_body(data),
    ])
}

fn stream_body(data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("<< /Length {} >>\nstream\n", data.len()).as_bytes());
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

fn assemble_classic(bodies: &[Vec<u8>]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = buf.len();
    let size = bodies.len() + 1;
    buf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF")
            .as_bytes(),
    );
    buf
}
