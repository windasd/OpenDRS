//! Supported radios: Kydera DR-8500UV / DRS-8500 and the re-badged HORA DH-98VU (port of the
//! non-GUI tests in tests/test_hardware.py).

mod common;

use common::fixtures;
use opendrs_core::f136;
use opendrs_firmware::hardware::{
    MODEL_NUMBER, REPORTED_MODEL, known_as, supported_names, supported_text,
};

#[test]
fn supported_list_names_the_dh98vu() {
    assert_eq!(
        supported_names(),
        "Kydera DR-8500UV／Kydera DRS-8500／HORA DH-98VU"
    );
    assert!(supported_text().contains("DRS-8500") && supported_text().contains("DR8600"));
}

#[test]
fn dh98vu_project_reports_the_shared_identifiers() {
    let dev = f136::load(fixtures().join("dh98vu_20250811.F136"))
        .unwrap()
        .device();
    assert_eq!(
        (dev.model.as_str(), dev.model_number.as_str()),
        (REPORTED_MODEL, MODEL_NUMBER)
    );
    assert_eq!(known_as(&dev.model), "Kydera DR-8500UV／HORA DH-98VU");
    assert_eq!(known_as("XYZ-1"), "");
    assert_eq!(known_as(""), "");
}
