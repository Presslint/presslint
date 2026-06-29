#![allow(missing_docs)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use presslint_actions::{Action, ConvertColor, Recipe, RecipeStep, plan_recipe};
use presslint_core::{ColorSpace, ContentScope, EditCapability, ObjectKind, PageIndex, PdfName};
use presslint_inventory::{Inventory, build_inventory};
use presslint_selectors::{Predicate, Selector, matches as selector_matches};
use presslint_syntax::{assemble_operators, tokenize};

// A small mixed stream: one default-color text show that becomes a
// `MissingColorSource` skip (the page-default `DeviceGray` fill has no
// color-operator source), followed by two sourced process-color fills that
// become `ConvertColor` targets with planned patches.
const SMALL_STREAM: &[u8] = br"
BT (Hello) Tj ET
q
0.1 0.2 0.3 rg
10 20 30 40 re f
0.2 0.3 0.4 0.1 k
5 5 15 15 re f
Q
";

// A target-heavy unit: every fill paints under a freshly sourced process color
// operand, so each repetition contributes three `ConvertColor` targets with
// planned patches and no skips.
const TARGET_HEAVY_UNIT: &[u8] = br"
q
0.1 0.2 0.3 rg
10 20 30 40 re f
0.2 0.3 0.4 0.1 k
5 5 15 15 re f
0.5 g
1 2 3 4 re f
Q
";

// A skip-heavy unit: two default-color text shows and one default-color stroke,
// all of which carry the page-default `DeviceGray` color with no source range,
// so each becomes a `MissingColorSource` skip during `ConvertColor` planning.
const SKIP_HEAVY_UNIT: &[u8] = br"
BT (A) Tj (B) Tj ET
10 20 m 30 40 l S
";

// A short tail with one sourced process-color fill, appended after the
// skip-heavy body so the many-skip case still exercises the target/patch branch
// alongside the dominant skip branch.
const FEW_TARGET_TAIL: &[u8] = br"
0.1 0.2 0.3 rg 1 2 3 4 re f
";

// A diverse unit for the selector-matching group: a sourced process-color fill
// (vector), a text show, an image `Do`, a form `Do`, and a sourced stroke
// (vector). This produces all four `ObjectKind`s so the matcher's predicates do
// real per-entry work.
const SELECTOR_UNIT: &[u8] = br"
q
0.1 0.2 0.3 rg
10 20 30 40 re f
BT (T) Tj ET
/Im1 Do
/Fm1 Do
0.4 G
10 20 m 30 40 l S
Q
";

struct PlanningCase {
    name: &'static str,
    inventory: Inventory,
    recipe: Recipe,
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

fn repeated(unit: &[u8], repetitions: usize) -> Vec<u8> {
    let mut stream = Vec::with_capacity(unit.len() * repetitions);
    for _ in 0..repetitions {
        stream.extend_from_slice(unit);
    }
    stream
}

fn repeated_with_tail(unit: &[u8], repetitions: usize, tail: &[u8]) -> Vec<u8> {
    let mut stream = repeated(unit, repetitions);
    stream.extend_from_slice(tail);
    stream
}

/// Build a content-ordered inventory from a synthetic stream, mirroring the
/// `presslint-inventory` bench construction: tokenize, assemble, then
/// `build_inventory`. Done once per case, outside any timed loop.
fn build_synthetic_inventory(
    source: &[u8],
    image_xobject_names: &[PdfName],
    form_xobject_names: &[PdfName],
) -> Inventory {
    let tokens = require_ok(tokenize(source), "synthetic stream tokenizes");
    let records = require_ok(assemble_operators(&tokens), "synthetic stream assembles").records;
    require_ok(
        build_inventory(
            source,
            &records,
            PageIndex(0),
            &ContentScope::Page,
            image_xobject_names,
            form_xobject_names,
        ),
        "synthetic inventory builds",
    )
}

/// A `ConvertColor` recipe selecting every entry, so the planner exercises both
/// the target/patch branch (sourced process-color fills) and the skip branch
/// (default-color shows and strokes) of `plan_recipe`.
fn convert_all_recipe() -> Recipe {
    Recipe {
        schema_version: 1,
        steps: vec![RecipeStep {
            select: Selector::All,
            action: Action::ConvertColor(ConvertColor {
                target: "pso-coated-v3".to_owned(),
            }),
        }],
    }
}

fn planning_cases() -> Vec<PlanningCase> {
    let no_names: [PdfName; 0] = [];
    vec![
        PlanningCase {
            name: "small_mixed",
            inventory: build_synthetic_inventory(SMALL_STREAM, &no_names, &no_names),
            recipe: convert_all_recipe(),
        },
        PlanningCase {
            name: "large_repeated_targets",
            inventory: build_synthetic_inventory(
                &repeated(TARGET_HEAVY_UNIT, 128),
                &no_names,
                &no_names,
            ),
            recipe: convert_all_recipe(),
        },
        PlanningCase {
            name: "many_skip_few_target",
            inventory: build_synthetic_inventory(
                &repeated_with_tail(SKIP_HEAVY_UNIT, 256, FEW_TARGET_TAIL),
                &no_names,
                &no_names,
            ),
            recipe: convert_all_recipe(),
        },
    ]
}

fn plan_recipe_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("actions/plan_recipe");
    for case in planning_cases() {
        group.throughput(Throughput::Elements(throughput_count(case.inventory.len())));
        group.bench_with_input(BenchmarkId::from_parameter(case.name), &case, |b, input| {
            b.iter(|| plan_recipe(black_box(&input.recipe), black_box(&input.inventory)));
        });
    }
    group.finish();
}

fn selector_matches_throughput(c: &mut Criterion) {
    // A large diverse inventory and a multi-predicate selector, so the matcher
    // walks real `ObjectKind`/`ColorSpace`/capability checks per entry.
    let image_xobject_names = [PdfName(b"Im1".to_vec())];
    let form_xobject_names = [PdfName(b"Fm1".to_vec())];
    let inventory = build_synthetic_inventory(
        &repeated(SELECTOR_UNIT, 256),
        &image_xobject_names,
        &form_xobject_names,
    );
    let selector = Selector::Or {
        exprs: vec![
            Selector::And {
                exprs: vec![
                    Selector::Predicate {
                        predicate: Predicate::ObjectKind {
                            object_kind: ObjectKind::Vector,
                        },
                    },
                    Selector::Predicate {
                        predicate: Predicate::ColorSpace {
                            space: ColorSpace::DeviceRgb,
                        },
                    },
                ],
            },
            Selector::Predicate {
                predicate: Predicate::ObjectKind {
                    object_kind: ObjectKind::Text,
                },
            },
            Selector::Predicate {
                predicate: Predicate::Editable {
                    capability: EditCapability::RewriteColorOperand,
                },
            },
        ],
    };

    let mut group = c.benchmark_group("actions/selector_matches");
    group.throughput(Throughput::Elements(throughput_count(inventory.len())));
    group.bench_function(BenchmarkId::from_parameter("large_mixed"), |b| {
        b.iter(|| {
            let mut matched = 0_usize;
            for entry in &inventory.entries {
                if selector_matches(black_box(&selector), black_box(entry)) {
                    matched += 1;
                }
            }
            matched
        });
    });
    group.finish();
}

criterion_group!(benches, plan_recipe_throughput, selector_matches_throughput);
criterion_main!(benches);
