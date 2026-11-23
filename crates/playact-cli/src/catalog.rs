//! Markdown catalog generated from a TreeSpec JSON Schema.

use std::collections::HashMap;

use serde_json::Value;

/// Renders node types and registered enum names as Markdown.
#[must_use]
pub fn render(schema: &Value) -> String {
    let mut output = String::from(
        "# Node catalog\n\n\
         Generated from a TreeSpec JSON Schema. A registry-aware schema also \
         lists registered names and their descriptions.\n",
    );
    let Some(variants) = schema
        .pointer("/$defs/SpecNode/oneOf")
        .and_then(Value::as_array)
    else {
        output.push_str("\nNo `$defs/SpecNode/oneOf` definition was found.\n");
        return output;
    };

    output.push_str("\n## Node types\n\n| Type | Description |\n| --- | --- |\n");
    for variant in variants {
        let kind = node_type(variant);
        let description = variant
            .get("description")
            .and_then(Value::as_str)
            .map_or_else(|| String::from("—"), cell);
        output.push_str("| ");
        output.push_str(&code_span(&cell(kind)));
        output.push_str(" | ");
        output.push_str(&description);
        output.push_str(" |\n");
    }

    let mut rows = Vec::new();
    for variant in variants {
        let kind = node_type(variant);
        let Some(properties) = variant.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for (field, property) in properties {
            if field == "type" {
                continue;
            }
            collect_names(&format!("{kind}.{field}"), property, &mut rows);
            if let Some(items) = property.get("items") {
                collect_names(&format!("{kind}.{field}[]"), items, &mut rows);
            }
        }
    }
    if let Some(properties) = schema
        .pointer("/$defs/UtilityBranchSpec/properties")
        .and_then(Value::as_object)
    {
        for (field, property) in properties {
            collect_names(
                &format!("utility_selector.branch.{field}"),
                property,
                &mut rows,
            );
        }
    }

    if !rows.is_empty() {
        rows.sort();
        output.push_str(
            "\n## Registered names\n\n| Field | Name | Description |\n| --- | --- | --- |\n",
        );
        for (field, name, description) in rows {
            let description = if description.is_empty() {
                String::from("—")
            } else {
                cell(&description)
            };
            output.push_str("| ");
            output.push_str(&code_span(&cell(&field)));
            output.push_str(" | ");
            output.push_str(&code_span(&cell(&name)));
            output.push_str(" | ");
            output.push_str(&description);
            output.push_str(" |\n");
        }
    }

    output
}

fn node_type(variant: &Value) -> &str {
    variant
        .pointer("/properties/type/const")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn collect_names(field: &str, property: &Value, rows: &mut Vec<(String, String, String)>) {
    let Some(values) = property.get("enum").and_then(Value::as_array) else {
        return;
    };
    let descriptions = descriptions(property.get("oneOf"));
    for value in values {
        let Some(name) = value.as_str() else {
            continue;
        };
        rows.push((
            field.to_string(),
            name.to_string(),
            descriptions.get(name).cloned().unwrap_or_default(),
        ));
    }
}

fn descriptions(one_of: Option<&Value>) -> HashMap<String, String> {
    one_of
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some((
                entry.get("const")?.as_str()?.to_string(),
                entry.get("description")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use playact::config::{NodeCategory, NodeRegistry};
    use playact::prelude::*;

    #[test]
    fn renders_built_in_node_types() {
        let catalog = render(&playact::config::TreeSpec::json_schema());
        assert!(catalog.contains("## Node types"));
        assert!(catalog.contains("`sequence`"));
        assert!(catalog.contains("`utility_selector`"));
    }

    #[test]
    fn renders_registered_names_and_descriptions() {
        let mut registry = NodeRegistry::<()>::new();
        registry
            .action::<()>("patrol", |_| |_: &mut Context<'_, ()>| Status::Success)
            .unwrap();
        registry
            .condition::<()>("always", |_| |_: &Context<'_, ()>| true)
            .unwrap();
        registry
            .describe(NodeCategory::Action, "patrol", "Walk the route.")
            .unwrap();
        registry
            .describe(NodeCategory::Condition, "always", "Always true.")
            .unwrap();

        let catalog = render(&registry.json_schema());
        assert!(catalog.contains("## Registered names"));
        assert!(catalog.contains("`action.name`"));
        assert!(catalog.contains("`patrol`"));
        assert!(catalog.contains("Walk the route."));
        assert!(catalog.contains("`condition.name`"));
        assert!(catalog.contains("`always`"));
    }

    #[test]
    fn reports_missing_spec_node_definition() {
        let catalog = render(&serde_json::json!({}));
        assert!(catalog.contains("No `$defs/SpecNode/oneOf` definition"));
    }
}
