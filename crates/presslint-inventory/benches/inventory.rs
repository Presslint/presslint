#![allow(missing_docs)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use presslint_core::{ContentScope, PageIndex, PdfName};
use presslint_inventory::{build_inventory, walk_graphics_state};
use presslint_syntax::{OperatorRecord, assemble_operators, tokenize};

const SMALL_STREAM: &[u8] = br"
q
0.1 0.2 0.3 rg
10 20 30 40 re f
BT /F1 12 Tf (Hello) Tj ET
/Im1 Do
/GS1 gs
Q
";

const REPEATED_STREAM_UNIT: &[u8] = br"
q
1 0 0 1 2 3 cm
0.1 0.2 0.3 rg
0.4 G
10 20 m 30 40 l S
5 5 15 15 re f
BT 0 Tr (Synthetic text) Tj [(A) 120 (B)] TJ ET
/Im1 Do
/Fm1 Do
/GS1 gs
n
Q
";

// Path-construction and no-op operators only: every record walks but none emits
// an inventory entry. Repeated many times, this dominates the stream with no-ops.
const NOOP_HEAVY_UNIT: &[u8] = br"
10 20 m
30 40 l
50 60 l
h
W
n
1 0 0 1 1 1 cm
";

// A short tail with a few entry-producing operators (one fill, one text show,
// one image Do, one form Do) appended after the no-op-heavy body.
const FEW_ENTRY_TAIL: &[u8] = br"
0.1 0.2 0.3 rg f
BT (Hi) Tj ET
/Im1 Do
/Fm1 Do
";

struct InventoryCase {
    name: &'static str,
    source: Vec<u8>,
    records: Vec<OperatorRecord>,
    image_xobject_names: Vec<PdfName>,
    form_xobject_names: Vec<PdfName>,
}

fn repeated_stream(repetitions: usize) -> Vec<u8> {
    let mut stream = Vec::with_capacity(REPEATED_STREAM_UNIT.len() * repetitions);
    for _ in 0..repetitions {
        stream.extend_from_slice(REPEATED_STREAM_UNIT);
    }
    stream
}

fn noop_heavy_stream(noop_repetitions: usize) -> Vec<u8> {
    let mut stream =
        Vec::with_capacity(NOOP_HEAVY_UNIT.len() * noop_repetitions + FEW_ENTRY_TAIL.len());
    for _ in 0..noop_repetitions {
        stream.extend_from_slice(NOOP_HEAVY_UNIT);
    }
    stream.extend_from_slice(FEW_ENTRY_TAIL);
    stream
}

fn require_ok<T, E: core::fmt::Debug>(result: Result<T, E>, context: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{context}: {error:?}");
            std::process::abort();
        }
    }
}

fn throughput_count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn assemble_source(source: &[u8]) -> Vec<OperatorRecord> {
    let tokens = require_ok(tokenize(source), "synthetic stream tokenizes");
    require_ok(assemble_operators(&tokens), "synthetic stream assembles").records
}

fn inventory_cases() -> [InventoryCase; 3] {
    [
        InventoryCase {
            name: "small_mixed_records",
            source: SMALL_STREAM.to_vec(),
            records: assemble_source(SMALL_STREAM),
            image_xobject_names: vec![PdfName(b"Im1".to_vec())],
            form_xobject_names: vec![PdfName(b"Fm1".to_vec())],
        },
        {
            let source = repeated_stream(128);
            let records = assemble_source(&source);
            InventoryCase {
                name: "large_repeated_records",
                source,
                records,
                image_xobject_names: vec![PdfName(b"Im1".to_vec())],
                form_xobject_names: vec![PdfName(b"Fm1".to_vec())],
            }
        },
        {
            // Many no-op/path-construction records with only four entries at the
            // tail: exercises combined `build_inventory` throughput when the
            // event stream is dominated by operators that emit no inventory
            // entry, the case the streaming driver is designed for.
            let source = noop_heavy_stream(512);
            let records = assemble_source(&source);
            InventoryCase {
                name: "many_noop_few_entry",
                source,
                records,
                image_xobject_names: vec![PdfName(b"Im1".to_vec())],
                form_xobject_names: vec![PdfName(b"Fm1".to_vec())],
            }
        },
    ]
}

fn walk_graphics_state_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("inventory/walk_graphics_state");
    for case in inventory_cases() {
        group.throughput(Throughput::Elements(throughput_count(case.records.len())));
        group.bench_with_input(BenchmarkId::from_parameter(case.name), &case, |b, input| {
            b.iter(|| {
                require_ok(
                    walk_graphics_state(black_box(&input.source), black_box(&input.records)),
                    "synthetic records walk",
                )
            });
        });
    }
    group.finish();
}

fn build_inventory_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("inventory/build_inventory");
    for case in inventory_cases() {
        let expected_entries = require_ok(
            build_inventory(
                &case.source,
                &case.records,
                PageIndex(0),
                &ContentScope::Page,
                &case.image_xobject_names,
                &case.form_xobject_names,
            ),
            "synthetic inventory builds",
        )
        .len();
        group.throughput(Throughput::Elements(throughput_count(expected_entries)));
        group.bench_with_input(BenchmarkId::from_parameter(case.name), &case, |b, input| {
            b.iter(|| {
                require_ok(
                    build_inventory(
                        black_box(&input.source),
                        black_box(&input.records),
                        PageIndex(0),
                        black_box(&ContentScope::Page),
                        black_box(&input.image_xobject_names),
                        black_box(&input.form_xobject_names),
                    ),
                    "synthetic inventory builds",
                )
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    walk_graphics_state_throughput,
    build_inventory_throughput
);
criterion_main!(benches);
