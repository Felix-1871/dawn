// SPDX-License-Identifier: GPL-3.0-or-later

//! Guards against the checked-in JSON schema drifting from the types it was
//! generated from. Regenerate with:
//! `cargo run -p plan --example gen_schema > plan/schema/install-plan.schema.json`

#[test]
fn checked_in_schema_matches_the_types() {
    let generated = serde_json::to_string_pretty(&plan::json_schema()).unwrap();
    let checked_in = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/schema/install-plan.schema.json"
    ))
    .expect("plan/schema/install-plan.schema.json should exist");

    assert_eq!(
        generated.trim(),
        checked_in.trim(),
        "\nschema/install-plan.schema.json is stale — regenerate it with:\n\
         cargo run -p plan --example gen_schema > plan/schema/install-plan.schema.json\n"
    );
}
