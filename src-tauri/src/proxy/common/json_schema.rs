use serde_json::Value;

/// Recursively clean JSON Schema to meet Gemini interface requirements
///
/// 1. [New] Expand $ref and $defs: Replace references with actual definitions, solving Gemini's lack of $ref support
/// 2. Remove unsupported fields: $schema, additionalProperties, format, default, uniqueItems, validation fields
/// 3. Handle union types: ["string", "null"] -> "string"
/// 4. Convert type field values to uppercase (Gemini v1internal requirement)
/// 5. Remove numeric validation fields: multipleOf, exclusiveMinimum, exclusiveMaximum etc.
pub fn clean_json_schema(value: &mut Value) {
    // 0. Pre-processing: Expand $ref (Schema Flattening)
    if let Value::Object(map) = value {
        let mut defs = serde_json::Map::new();
        // Extract $defs or definitions
        if let Some(Value::Object(d)) = map.remove("$defs") {
            defs.extend(d);
        }
        if let Some(Value::Object(d)) = map.remove("definitions") {
            defs.extend(d);
        }

        if !defs.is_empty() {
            // Recursively replace references
            flatten_refs(map, &defs);
        }
    }

    // Recursive cleaning
    clean_json_schema_recursive(value);
}

/// Recursively expand $ref
fn flatten_refs(map: &mut serde_json::Map<String, Value>, defs: &serde_json::Map<String, Value>) {
    // Check and replace $ref
    if let Some(Value::String(ref_path)) = map.remove("$ref") {
        // Parse reference name (e.g. #/$defs/MyType -> MyType)
        let ref_name = ref_path.split('/').last().unwrap_or(&ref_path);

        if let Some(def_schema) = defs.get(ref_name) {
            // Merge defined content into current map
            if let Value::Object(def_map) = def_schema {
                for (k, v) in def_map {
                    // Insert only if current map doesn't have this key (avoid overwrite)
                    // But usually $ref node shouldn't have other properties
                    map.entry(k.clone()).or_insert_with(|| v.clone());
                }

                // Recursively process $ref that might be contained in the just merged content
                // Note: This might recurse infinitely if there are circular references, but tool definitions are usually DAGs
                flatten_refs(map, defs);
            }
        }
    }

    // Traverse child nodes
    for (_, v) in map.iter_mut() {
        if let Value::Object(child_map) = v {
            flatten_refs(child_map, defs);
        } else if let Value::Array(arr) = v {
            for item in arr {
                if let Value::Object(item_map) = item {
                    flatten_refs(item_map, defs);
                }
            }
        }
    }
}

fn clean_json_schema_recursive(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // 1. Recursively process all child nodes first, ensuring nested structures are correctly cleaned
            for v in map.values_mut() {
                clean_json_schema_recursive(v);
            }

            // 2. Collect and process validation fields (Soft-Remove with Type Check & Unwrapping)
            let mut constraints = Vec::new();

            // String type validation (pattern): Must be String, otherwise it might be a property definition
            let string_validations = [("pattern", "pattern")];
            for (field, label) in string_validations {
                if let Some(val) = map.remove(field) {
                    if let Value::String(s) = val {
                        constraints.push(format!("{}: {}", label, s));
                    } else {
                        // Not String (e.g. it's a property definition of Object type), put it back
                        map.insert(field.to_string(), val);
                    }
                }
            }

            // Number type validation
            let number_validations = [
                ("minLength", "minLen"),
                ("maxLength", "maxLen"),
                ("minimum", "min"),
                ("maximum", "max"),
                ("minItems", "minItems"),
                ("maxItems", "maxItems"),
                ("exclusiveMinimum", "exclMin"),
                ("exclusiveMaximum", "exclMax"),
                ("multipleOf", "multipleOf"),
            ];
            for (field, label) in number_validations {
                if let Some(val) = map.remove(field) {
                    if val.is_number() {
                        constraints.push(format!("{}: {}", label, val));
                    } else {
                        // Not Number, put it back
                        map.insert(field.to_string(), val);
                    }
                }
            }

            // 3. Append constraint info to description
            if !constraints.is_empty() {
                let suffix = format!(" [Validation: {}]", constraints.join(", "));
                let desc = map
                    .entry("description".to_string())
                    .or_insert_with(|| Value::String("".to_string()));
                if let Value::String(s) = desc {
                    s.push_str(&suffix);
                }
            }

            // 4. Remove other non-standard/conflicting fields that would interfere with upstream
            let other_fields_to_remove = [
                "$schema",
                "additionalProperties",
                "enumCaseInsensitive",
                "enumNormalizeWhitespace",
                "uniqueItems",
                "format",
                "default",
                // Advanced fields commonly used by MCP tools but not supported by Gemini
                "propertyNames",
                "const",
                "anyOf",
                "oneOf",
                "allOf",
                "not",
                "if",
                "then",
                "else",
            ];
            for field in other_fields_to_remove {
                map.remove(field);
            }

            // 5. Handle type field (Gemini Protobuf doesn't support array types, force downgrade)
            if let Some(type_val) = map.get_mut("type") {
                match type_val {
                    Value::String(s) => {
                        *type_val = Value::String(s.to_lowercase());
                    }
                    Value::Array(arr) => {
                        // Handle ["string", "null"] -> select first non-null string
                        // Any array type must be downgraded to a single type
                        let mut selected_type = "string".to_string();
                        for item in arr {
                            if let Value::String(s) = item {
                                if s != "null" {
                                    selected_type = s.to_lowercase();
                                    break;
                                }
                            }
                        }
                        *type_val = Value::String(selected_type);
                    }
                    _ => {}
                }
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                clean_json_schema_recursive(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_clean_json_schema_draft_2020_12() {
        let mut schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "minLength": 1,
                    "format": "city"
                },
                // Simulate property name conflict: pattern is an Object property, should not be removed
                "pattern": {
                    "type": "object",
                    "properties": {
                        "regex": { "type": "string", "pattern": "^[a-z]+$" }
                    }
                },
                "unit": {
                    "type": ["string", "null"],
                    "default": "celsius"
                }
            },
            "required": ["location"]
        });

        clean_json_schema(&mut schema);

        // 1. Verify type remains lowercase
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["location"]["type"], "string");

        // 2. Verify standard fields are converted and moved to description (Advanced Soft-Remove)
        assert!(schema["properties"]["location"].get("minLength").is_none());
        assert!(schema["properties"]["location"]["description"]
            .as_str()
            .unwrap()
            .contains("minLen: 1"));

        // 3. Verify property named "pattern" is not accidentally removed
        assert!(schema["properties"].get("pattern").is_some());
        assert_eq!(schema["properties"]["pattern"]["type"], "object");

        // 4. Verify internal pattern validation field is correctly removed and converted to description
        assert!(schema["properties"]["pattern"]["properties"]["regex"]
            .get("pattern")
            .is_none());
        assert!(
            schema["properties"]["pattern"]["properties"]["regex"]["description"]
                .as_str()
                .unwrap()
                .contains("pattern: ^[a-z]+$")
        );

        // 5. Verify union types are downgraded to single type (Protobuf compatibility)
        assert_eq!(schema["properties"]["unit"]["type"], "string");

        // 6. Verify metadata fields are removed
        assert!(schema.get("$schema").is_none());
    }

    #[test]
    fn test_type_fallback() {
        // Test ["string", "null"] -> "string"
        let mut s1 = json!({"type": ["string", "null"]});
        clean_json_schema(&mut s1);
        assert_eq!(s1["type"], "string");

        // Test ["integer", "null"] -> "integer" (and lowercase check if needed, though usually integer)
        let mut s2 = json!({"type": ["integer", "null"]});
        clean_json_schema(&mut s2);
        assert_eq!(s2["type"], "integer");
    }

    #[test]
    fn test_flatten_refs() {
        let mut schema = json!({
            "$defs": {
                "Address": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                }
            },
            "properties": {
                "home": { "$ref": "#/$defs/Address" }
            }
        });

        clean_json_schema(&mut schema);

        // Verify references are expanded and types converted to lowercase
        assert_eq!(schema["properties"]["home"]["type"], "object");
        assert_eq!(
            schema["properties"]["home"]["properties"]["city"]["type"],
            "string"
        );
    }
}
