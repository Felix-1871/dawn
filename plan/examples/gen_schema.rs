// SPDX-License-Identifier: GPL-3.0-or-later

//! Regenerates `plan/schema/install-plan.schema.json` from the current
//! `InstallPlan` types. Run after changing the plan shape:
//!
//! ```sh
//! cargo run -p plan --example gen_schema > plan/schema/install-plan.schema.json
//! ```

fn main() {
    let schema = plan::json_schema();
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}
