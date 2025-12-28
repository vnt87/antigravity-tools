// Claude Helper Functions
// JSON Schema cleaning, signature handling, etc.

// Removed unused Value import

/// Convert type names in JSON Schema to uppercase (Gemini requirement)
/// e.g.: "string" -> "STRING", "integer" -> "INTEGER"
// Removed unused uppercase_schema_types function

/// Convert Gemini UsageMetadata to Claude Usage
pub fn to_claude_usage(usage_metadata: &super::models::UsageMetadata) -> super::models::Usage {
    super::models::Usage {
        input_tokens: usage_metadata.prompt_token_count.unwrap_or(0),
        output_tokens: usage_metadata.candidates_token_count.unwrap_or(0),
    }
}

/// Extract thoughtSignature
// Removed unused extract_thought_signature function

#[cfg(test)]
mod tests {
    use super::*;
    // Removed unused serde_json::json

    // Removed outdated tests for uppercase_schema_types

    #[test]
    fn test_to_claude_usage() {
        use super::super::models::UsageMetadata;

        let usage = UsageMetadata {
            prompt_token_count: Some(100),
            candidates_token_count: Some(50),
            total_token_count: Some(150),
        };

        let claude_usage = to_claude_usage(&usage);
        assert_eq!(claude_usage.input_tokens, 100);
        assert_eq!(claude_usage.output_tokens, 50);
    }
}
