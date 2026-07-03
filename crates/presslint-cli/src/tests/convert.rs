use std::fs;

use crate::{args::ConvertArgs, convert, report::ReportFormat};

const RGB_TO_CMYK_LINK: &str = "000001746c636d73043000006c696e6b52474220434d594b07ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d730000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014132423000000090000000e46d4142200000000003040000000000a40000000000000000000000500000002070617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000002020200000000000000000000000000020000000005002a004f0074009900be00e30108012d01520177019c01c101e6020b0230025500220047006c009100b600db01000125014a016f019401b901de0203022870617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000070617261000000000000000000010000";

#[test]
fn convert_run_writes_output_and_renders_report() {
    let dir = temp_dir("presslint-cli-convert");
    fs::create_dir_all(&dir).unwrap();
    let input_path = dir.join("input.pdf");
    let output_path = dir.join("output.pdf");
    let link_path = dir.join("synthetic-rgb-cmyk.icc");
    let input = classic_raw_pdf(b"1 0 0 rg\n");

    fs::write(&input_path, &input).unwrap();
    fs::write(&link_path, link_bytes(RGB_TO_CMYK_LINK)).unwrap();

    let args = ConvertArgs {
        input: input_path,
        device_links: vec![format!("rgb={}", link_path.display())],
        select: None,
        pages: "all".to_owned(),
        preserve_black: false,
        json: false,
        output: output_path.to_string_lossy().into_owned(),
    };

    let report = convert::run(&args).unwrap();
    report.render(ReportFormat::Human).unwrap();

    let written = fs::read(&output_path).unwrap();
    assert!(written.starts_with(&input));
    assert!(written.len() > input.len());

    let _ = fs::remove_file(&args.input);
    let _ = fs::remove_file(output_path);
    let _ = fs::remove_file(link_path);
    let _ = fs::remove_dir(dir);
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
