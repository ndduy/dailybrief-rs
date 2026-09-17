//! The Editor's final message (`SPEC.md` §4): validated by the harness against `schemas/digest.json`,
//! which `gen-schemas` regenerates from this type (draft-07, no extra properties).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const NOTES_MAX_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DigestOutput {
    /// The id returned by publish_digest
    #[schemars(length(min = 1))]
    pub digest_id: String,
    /// Digest date, YYYY-MM-DD, as returned by publish_digest
    #[schemars(regex(pattern = r"^\d{4}-\d{2}-\d{2}$"))]
    pub date: String,
    /// Number of for_you items published
    pub for_you: u32,
    /// Number of beyond_radar items published
    pub beyond_radar: u32,
    /// Optional one-line note for the human reader (≤ 300 chars)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 300))]
    pub notes: Option<String>,
}

/// The draft-07 schema the harness receives via `--json-schema`. Optional fields are
/// "omit or a string", never `null`: the null alternative schemars adds for `Option` is stripped.
pub fn schema() -> serde_json::Value {
    let schema = schemars::generate::SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<DigestOutput>();
    let mut value = serde_json::to_value(schema).unwrap_or(serde_json::Value::Null);
    strip_null_types(&mut value);
    value
}

/// `"type": ["string", "null"]` → `"type": "string"` on every property.
fn strip_null_types(schema: &mut serde_json::Value) {
    let Some(props) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    for prop in props.values_mut() {
        let Some(types) = prop.get("type").and_then(serde_json::Value::as_array) else {
            continue;
        };
        let kept: Vec<serde_json::Value> = types
            .iter()
            .filter(|t| t.as_str() != Some("null"))
            .cloned()
            .collect();
        prop["type"] = match kept.as_slice() {
            [single] => single.clone(),
            _ => serde_json::Value::Array(kept),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_output_rejects_unknown_fields() {
        let bad = json!({ "digestId": "x", "date": "2026-09-17", "forYou": 24, "beyondRadar": 6, "extra": 1 });
        assert!(serde_json::from_value::<DigestOutput>(bad).is_err());
        let ok = json!({ "digestId": "x", "date": "2026-09-17", "forYou": 24, "beyondRadar": 6 });
        let parsed: DigestOutput = serde_json::from_value(ok).unwrap();
        assert_eq!(parsed.notes, None);
    }

    #[test]
    fn schema_is_draft07_with_additional_properties_false_and_date_pattern() {
        let s = schema();
        assert_eq!(s["$schema"], "http://json-schema.org/draft-07/schema#");
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(
            s["required"],
            json!(["digestId", "date", "forYou", "beyondRadar"])
        );
        assert_eq!(s["properties"]["date"]["pattern"], r"^\d{4}-\d{2}-\d{2}$");
        assert_eq!(s["properties"]["digestId"]["minLength"], 1);
        assert_eq!(s["properties"]["notes"]["maxLength"], 300);
        assert_eq!(
            s["properties"]["notes"]["type"], "string",
            "optional means omit, never null"
        );
        assert_eq!(s["properties"]["forYou"]["type"], "integer");
        assert!(
            s["properties"]["digestId"]["description"]
                .as_str()
                .unwrap()
                .contains("publish_digest")
        );
    }
}
