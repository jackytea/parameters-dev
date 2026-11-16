use schemars::generate::SchemaSettings;

use crate::document::Workflow;

#[must_use]
#[expect(
    clippy::expect_used,
    reason = "generated schemas only contain string keys, so serialization cannot fail"
)]
pub fn workflow_schema() -> String {
    let schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<Workflow>();
    let mut text = serde_json::to_string_pretty(&schema).expect("schema serializes to JSON");
    text.push('\n');
    text
}
