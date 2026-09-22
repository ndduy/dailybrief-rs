//! The Curator's final message (`spec/m3.md` §8): validated by the harness against
//! `schemas/curator.json`, which `gen-schemas` regenerates from this type.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const NOTES_MAX_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CuratorOutput {
    /// The run id, when known (may be empty)
    #[serde(default)]
    pub run_id: String,
    /// Every proposal id propose_change returned this run, in order
    pub proposals: Vec<String>,
    /// Optional note for the human reader (≤ 500 chars)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 500))]
    pub notes: Option<String>,
}

/// The draft-07 schema the harness receives via `--json-schema` (optional fields: omit or a
/// string, never `null`).
pub fn schema() -> serde_json::Value {
    let schema = schemars::generate::SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<CuratorOutput>();
    let mut value = serde_json::to_value(schema).unwrap_or(serde_json::Value::Null);
    super::digest_output::strip_null_types(&mut value);
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn curator_output_parses_an_empty_list_and_rejects_unknown_fields() {
        let ok: CuratorOutput =
            serde_json::from_value(json!({ "runId": "r", "proposals": [], "notes": "quiet week" }))
                .unwrap();
        assert!(ok.proposals.is_empty());
        let no_run: CuratorOutput =
            serde_json::from_value(json!({ "proposals": ["p-1"] })).unwrap();
        assert_eq!(no_run.run_id, "");
        assert!(
            serde_json::from_value::<CuratorOutput>(json!({ "proposals": [], "x": 1 })).is_err()
        );
    }

    #[test]
    fn schema_is_draft07_with_the_notes_cap() {
        let s = schema();
        assert_eq!(s["$schema"], "http://json-schema.org/draft-07/schema#");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["required"], json!(["proposals"]));
        assert_eq!(s["properties"]["notes"]["maxLength"], 500);
        assert_eq!(s["properties"]["notes"]["type"], "string");
        assert_eq!(s["properties"]["proposals"]["type"], "array");
    }
}
