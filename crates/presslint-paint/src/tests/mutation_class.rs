//! `MutationClass` routing-predicate tests (Phase 0a-7).

use crate::MutationClass;

#[test]
fn mutation_class_preserves_source_bytes_for_verbatim_routes() {
    assert!(MutationClass::PreserveBytes.preserves_source_bytes());
    assert!(!MutationClass::SurgicalRewrite.preserves_source_bytes());
    assert!(!MutationClass::AppearanceReplacement.preserves_source_bytes());
    assert!(MutationClass::UnsupportedSkip.preserves_source_bytes());
}

#[test]
fn mutation_class_may_emit_replacement_bytes_for_rewrite_routes() {
    assert!(!MutationClass::PreserveBytes.may_emit_replacement_bytes());
    assert!(MutationClass::SurgicalRewrite.may_emit_replacement_bytes());
    assert!(MutationClass::AppearanceReplacement.may_emit_replacement_bytes());
    assert!(!MutationClass::UnsupportedSkip.may_emit_replacement_bytes());
}
