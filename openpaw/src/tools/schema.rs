use serde_json::{Map, Value};

pub enum CleaningStrategy {
    Gemini,
    Anthropic,
    OpenAi,
    Conservative,
}

pub struct SchemaCleanr;

impl SchemaCleanr {
    pub fn clean_for_gemini(schema_json: &str) -> Option<String> {
        Self::clean(schema_json, CleaningStrategy::Gemini)
    }

    pub fn validate(schema_json: &str) -> bool {
        if let Ok(val) = serde_json::from_str::<Value>(schema_json) {
            if let Some(obj) = val.as_object() {
                return obj.contains_key("type");
            }
        }
        false
    }

    pub fn clean(schema_json: &str, strategy: CleaningStrategy) -> Option<String> {
        let mut val: Value = serde_json::from_str(schema_json).ok()?;
        Self::clean_value(&mut val, &strategy);
        serde_json::to_string(&val).ok()
    }

    fn clean_value(val: &mut Value, strategy: &CleaningStrategy) {
        match val {
            Value::Object(map) => {
                let keys_to_remove = Self::unsupported_keywords(strategy);
                for key in keys_to_remove {
                    map.remove(key);
                }

                let mut vals_to_clean = Vec::new();
                for (k, v) in map.iter_mut() {
                    vals_to_clean.push(v);
                }
                for v in vals_to_clean {
                    Self::clean_value(v, strategy);
                }
            }
            Value::Array(arr) => {
                for v in arr.iter_mut() {
                    Self::clean_value(v, strategy);
                }
            }
            _ => {}
        }
    }

    fn unsupported_keywords(strategy: &CleaningStrategy) -> Vec<&'static str> {
        match strategy {
            CleaningStrategy::Gemini => vec![
                "$ref",
                "$schema",
                "$id",
                "$defs",
                "definitions",
                "additionalProperties",
                "patternProperties",
                "minLength",
                "maxLength",
                "pattern",
                "format",
                "minimum",
                "maximum",
                "multipleOf",
                "minItems",
                "maxItems",
                "uniqueItems",
                "minProperties",
                "maxProperties",
                "examples",
            ],
            CleaningStrategy::Anthropic => vec!["$ref", "$defs", "definitions"],
            CleaningStrategy::OpenAi => vec![],
            CleaningStrategy::Conservative => {
                vec!["$ref", "$defs", "definitions", "additionalProperties"]
            }
        }
    }
}
