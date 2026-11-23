//! Markdown node catalog generated from registry metadata.

use std::collections::HashSet;

use serde_json::Value;

/// One registered name and the metadata attached to it.
pub(crate) struct CatalogEntry {
    pub name: String,
    pub description: Option<String>,
    pub schema: Option<Value>,
}

/// One rendered category.
pub(crate) struct CatalogSection<'a> {
    pub title: &'a str,
    pub column: &'a str,
    pub entries: Vec<CatalogEntry>,
}

/// Renders the catalog sections as Markdown.
pub(crate) fn render(sections: &[CatalogSection<'_>]) -> String {
    let mut output = String::from("# Node catalog\n\n");
    output.push_str(
        "Generated from a `NodeRegistry`. Descriptions come from `describe`; \
         parameter and value summaries come from `describe_params`, \
         `describe_switch_key`, and `describe_set_value_key`.\n",
    );

    if sections.iter().all(|section| section.entries.is_empty()) {
        output.push_str("\nNo nodes are registered.\n");
        return output;
    }

    for section in sections {
        if section.entries.is_empty() {
            continue;
        }
        output.push_str("\n## ");
        output.push_str(section.title);
        output.push_str("\n\n| Name | Description | ");
        output.push_str(section.column);
        output.push_str(" |\n| --- | --- | --- |\n");
        for entry in &section.entries {
            let description = entry
                .description
                .as_deref()
                .map_or_else(|| String::from("—"), cell);
            let details = entry
                .schema
                .as_ref()
                .map_or_else(|| String::from("—"), summarize);

            output.push_str("| ");
            output.push_str(&code_span(&cell(&entry.name)));
            output.push_str(" | ");
            output.push_str(&description);
            output.push_str(" | ");
            output.push_str(&details);
            output.push_str(" |\n");
        }
    }
    output
}

/// Summarizes an object schema as field list, or any other schema as a type.
fn summarize(schema: &Value) -> String {
    summarize_at(schema, schema, 0)
}

fn summarize_at(schema: &Value, root: &Value, depth: usize) -> String {
    if depth > 8 {
        return String::from("object");
    }
    let schema = resolve_ref(schema, root).unwrap_or(schema);
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return type_label(schema, root, depth);
    };
    if properties.is_empty() {
        return String::from("object");
    }

    let required: HashSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let fields: Vec<String> = properties
        .iter()
        .map(|(name, field)| {
            let field_type = type_label(field, root, depth + 1);
            let requirement = if required.contains(name.as_str()) {
                ", required"
            } else {
                ""
            };
            let description = field
                .get("description")
                .and_then(Value::as_str)
                .map(|description| format!(" — {}", cell(description)))
                .unwrap_or_default();
            format!(
                "{} ({field_type}{requirement}){description}",
                code_span(&cell(name))
            )
        })
        .collect();
    fields.join("<br>")
}

fn type_label(schema: &Value, root: &Value, depth: usize) -> String {
    if depth > 8 {
        return String::from("object");
    }
    let schema = resolve_ref(schema, root).unwrap_or(schema);

    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let labels: Vec<String> = values.iter().map(value_label).collect();
        return format!("enum: {}", labels.join(" | "));
    }
    if let Some(variants) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        let mut labels: Vec<String> = variants
            .iter()
            .map(|variant| type_label(variant, root, depth + 1))
            .collect();
        labels.sort();
        labels.dedup();
        return labels.join(" | ");
    }
    if let Some(schema_type) = schema.get("type") {
        if let Some(schema_type) = schema_type.as_str() {
            if schema_type == "array" {
                let items = schema.get("items").map_or_else(
                    || String::from("any"),
                    |items| type_label(items, root, depth + 1),
                );
                return format!("array<{items}>");
            }
            return String::from(schema_type);
        }
        if let Some(types) = schema_type.as_array() {
            let labels: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            return labels.join(" | ");
        }
    }
    if schema.get("properties").is_some() || schema.get("additionalProperties").is_some() {
        return String::from("object");
    }
    String::from("any")
}

fn value_label(value: &Value) -> String {
    match value {
        Value::String(value) => code_span(&cell(value)),
        Value::Null => String::from("null"),
        other => cell(&other.to_string()),
    }
}

fn resolve_ref<'a>(schema: &'a Value, root: &'a Value) -> Option<&'a Value> {
    let reference = schema.get("$ref")?.as_str()?;
    root.pointer(reference.strip_prefix('#')?)
}

/// Escapes table cell content: pipes separate columns and newlines break rows.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}

/// Wraps text in a code span, widening the fence when the text contains one.
fn code_span(text: &str) -> String {
    let mut fence = String::from("`");
    while text.contains(&fence) {
        fence.push('`');
    }
    if text.starts_with('`') || text.ends_with('`') {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}
