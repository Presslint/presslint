//! Walker + inventory tests for page resource colour-space tracking.
//!
//! These exercise `cs`/`CS` + `sc`/`scn`/`SC`/`SCN` over `ICCBased`,
//! `Separation`, and `DeviceN` resource spaces resolved against a borrowed
//! colour-space environment, the initial-colour-after-`cs` case, unresolved
//! names, and the device-operator regression (an environment must not perturb
//! device colours).

use presslint_syntax::{assemble_operators, tokenize};
use presslint_types::{ColorSpace, ColorUsage, ContentScope, PdfName};

use crate::{
    ColorSpaceEnv, ColorSpaceResource, Inventory, build_inventory,
    build_inventory_with_color_space_env,
};

fn name(bytes: &[u8]) -> PdfName {
    PdfName(bytes.to_vec())
}

fn icc(resource: &[u8], components: usize) -> ColorSpaceResource {
    ColorSpaceResource {
        name: name(resource),
        space: ColorSpace::IccBased,
        component_count: Some(components),
        spot_names: Vec::new(),
    }
}

fn separation(resource: &[u8], colorant: &[u8]) -> ColorSpaceResource {
    ColorSpaceResource {
        name: name(resource),
        space: ColorSpace::Separation,
        component_count: Some(1),
        spot_names: vec![name(colorant)],
    }
}

fn device_n(resource: &[u8], colorants: &[&[u8]]) -> ColorSpaceResource {
    ColorSpaceResource {
        name: name(resource),
        space: ColorSpace::DeviceN,
        component_count: Some(colorants.len()),
        spot_names: colorants.iter().map(|c| name(c)).collect(),
    }
}

fn inventory_with_env(input: &[u8], resources: &[ColorSpaceResource]) -> Result<Inventory, String> {
    let tokens = tokenize(input).map_err(|error| format!("tokenize: {error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("assemble: {error:?}"))?;
    build_inventory_with_color_space_env(
        input,
        &assembled.records,
        presslint_types::PageIndex(0),
        &ContentScope::Page,
        &[],
        &[],
        ColorSpaceEnv::new(resources),
    )
    .map_err(|error| format!("inventory: {error:?}"))
}

#[test]
fn icc_based_n4_scn_is_reported_as_icc_never_cmyk() -> Result<(), String> {
    let inventory = inventory_with_env(
        b"/CS0 cs 0.1 0.2 0.3 0.4 scn 0 0 1 1 re f",
        &[icc(b"CS0", 4)],
    )?;
    assert_eq!(inventory.len(), 1);
    let color = &inventory.entries[0].colors[0];
    assert_eq!(color.usage, ColorUsage::Fill);
    assert_eq!(color.space, ColorSpace::IccBased);
    assert_ne!(color.space, ColorSpace::DeviceCmyk);
    assert_eq!(color.components, vec![0.1, 0.2, 0.3, 0.4]);
    assert_eq!(color.spot_name, None);
    assert!(color.source.is_some());
    Ok(())
}

#[test]
fn separation_scn_populates_spot_name() -> Result<(), String> {
    let inventory = inventory_with_env(
        b"/CS1 cs 0.5 scn 0 0 1 1 re f",
        &[separation(b"CS1", b"PANTONE")],
    )?;
    let color = &inventory.entries[0].colors[0];
    assert_eq!(color.space, ColorSpace::Separation);
    assert_eq!(color.components, vec![0.5]);
    assert_eq!(color.spot_name, Some(name(b"PANTONE")));
    Ok(())
}

#[test]
fn device_n_scn_reports_first_colorant() -> Result<(), String> {
    let inventory = inventory_with_env(
        b"/CS2 cs 0.1 0.2 scn 0 0 1 1 re f",
        &[device_n(b"CS2", &[b"Cut", b"Varnish"])],
    )?;
    let color = &inventory.entries[0].colors[0];
    assert_eq!(color.space, ColorSpace::DeviceN);
    assert_eq!(color.components, vec![0.1, 0.2]);
    assert_eq!(color.spot_name, Some(name(b"Cut")));
    Ok(())
}

#[test]
fn paint_after_cs_before_scn_reports_initial_colour() -> Result<(), String> {
    let inventory = inventory_with_env(b"/CS0 cs 0 0 1 1 re f", &[icc(b"CS0", 3)])?;
    let color = &inventory.entries[0].colors[0];
    assert_eq!(color.space, ColorSpace::IccBased);
    // Implied ICC initial colour is all-zero, never a stale device colour.
    assert_eq!(color.components, vec![0.0, 0.0, 0.0]);
    // The source points at the `cs` record, not a device operator.
    assert!(color.source.is_some());
    Ok(())
}

#[test]
fn separation_initial_colour_is_full_tint() -> Result<(), String> {
    let inventory = inventory_with_env(b"/CS1 cs 0 0 1 1 re f", &[separation(b"CS1", b"Spot")])?;
    let color = &inventory.entries[0].colors[0];
    assert_eq!(color.space, ColorSpace::Separation);
    assert_eq!(color.components, vec![1.0]);
    assert_eq!(color.spot_name, Some(name(b"Spot")));
    Ok(())
}

#[test]
fn unresolved_name_is_resource_not_unknown() -> Result<(), String> {
    let inventory = inventory_with_env(b"/CSX cs 0.5 scn 0 0 1 1 re f", &[icc(b"CS0", 3)])?;
    let color = &inventory.entries[0].colors[0];
    assert_eq!(color.space, ColorSpace::Resource(name(b"CSX")));
    assert_ne!(color.space, ColorSpace::Unknown);
    Ok(())
}

#[test]
fn stroking_side_uses_uppercase_operators() -> Result<(), String> {
    let inventory = inventory_with_env(b"/CS0 CS 0.4 SCN S", &[icc(b"CS0", 1)])?;
    let color = &inventory.entries[0].colors[0];
    assert_eq!(color.usage, ColorUsage::Stroke);
    assert_eq!(color.space, ColorSpace::IccBased);
    assert_eq!(color.components, vec![0.4]);
    Ok(())
}

#[test]
fn resource_colour_travels_through_save_restore() -> Result<(), String> {
    let inventory = inventory_with_env(
        b"/CS1 cs 0.5 scn q 0 g Q 0 0 1 1 re f",
        &[separation(b"CS1", b"Spot")],
    )?;
    // The `q ... Q` block set and restored a device grey; the restored fill is
    // the Separation colour established before the save.
    let color = &inventory.entries[0].colors[0];
    assert_eq!(color.space, ColorSpace::Separation);
    assert_eq!(color.components, vec![0.5]);
    Ok(())
}

#[test]
fn device_operators_are_unchanged_by_a_populated_environment() -> Result<(), String> {
    let content = b"0 0 0 1 k 0 0 1 1 re f";
    let with_env = inventory_with_env(content, &[icc(b"CS0", 4)])?;
    let tokens = tokenize(content).map_err(|error| format!("tokenize: {error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("assemble: {error:?}"))?;
    let device_only = build_inventory(
        content,
        &assembled.records,
        presslint_types::PageIndex(0),
        &ContentScope::Page,
        &[],
        &[],
    )
    .map_err(|error| format!("inventory: {error:?}"))?;
    // A device-only stream is byte-identical whether or not a colour-space
    // environment is present.
    assert_eq!(with_env, device_only);
    let color = &with_env.entries[0].colors[0];
    assert_eq!(color.space, ColorSpace::DeviceCmyk);
    assert_eq!(color.components, vec![0.0, 0.0, 0.0, 1.0]);
    Ok(())
}
