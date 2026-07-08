use std::cmp::Ordering;
use std::fmt::Write;

/// A scalar or composite value produced by evaluating a Cypher expression or projecting a result.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CypherValue {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    /// A graph node. `id` is the provider's stable, opaque identity for the node (used for equality
    /// and grouping, never displayed); `label` and `name` are what gets rendered.
    Node {
        id: String,
        label: String,
        name: String,
    },
    List(Vec<CypherValue>),
    /// An ordered map, e.g. from a map projection `n { .name, k: expr }`.
    Map(Vec<(String, CypherValue)>),
}

impl CypherValue {
    /// Returns the truthiness of a value for use in `WHERE` filtering.
    /// `NULL` and `false` are falsy; everything else (including bound nodes) is truthy.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            CypherValue::Null => false,
            CypherValue::Bool(b) => *b,
            _ => true,
        }
    }

    /// Returns a numeric view of the value if it is an integer.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            CypherValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Returns a string view of the value if it is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            CypherValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Rank of each variant for cross-type ordering, following openCypher orderability:
    /// `Map < Node < List < String < Boolean < Number < null` (ascending). `null` sorts last;
    /// strings sort before numbers.
    fn type_rank(&self) -> u8 {
        match self {
            CypherValue::Map(_) => 0,
            CypherValue::Node { .. } => 1,
            CypherValue::List(_) => 2,
            CypherValue::Str(_) => 3,
            CypherValue::Bool(_) => 4,
            CypherValue::Int(_) => 5,
            CypherValue::Null => 6,
        }
    }

    /// Total ordering across all value types, following openCypher orderability. Values of the same
    /// type are ordered by value; values of differing types by [`Self::type_rank`], so `null` sorts
    /// last and strings sort before numbers. (Nodes are ordered by name rather than identity.)
    #[must_use]
    pub fn total_cmp(&self, other: &CypherValue) -> Ordering {
        match (self, other) {
            (CypherValue::Bool(a), CypherValue::Bool(b)) => a.cmp(b),
            (CypherValue::Int(a), CypherValue::Int(b)) => a.cmp(b),
            (CypherValue::Str(a), CypherValue::Str(b))
            | (CypherValue::Node { name: a, .. }, CypherValue::Node { name: b, .. }) => a.cmp(b),
            (CypherValue::List(a), CypherValue::List(b)) => {
                for (x, y) in a.iter().zip(b.iter()) {
                    let ordering = x.total_cmp(y);
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                a.len().cmp(&b.len())
            }
            (CypherValue::Map(a), CypherValue::Map(b)) => {
                for ((ak, av), (bk, bv)) in a.iter().zip(b.iter()) {
                    let ordering = ak.cmp(bk).then_with(|| av.total_cmp(bv));
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                a.len().cmp(&b.len())
            }
            _ => self.type_rank().cmp(&other.type_rank()),
        }
    }

    /// Renders the value for display in a plain-text table cell.
    #[must_use]
    pub fn to_display_string(&self) -> String {
        match self {
            CypherValue::Null => String::new(),
            CypherValue::Bool(b) => b.to_string(),
            CypherValue::Int(i) => i.to_string(),
            CypherValue::Str(s) => s.clone(),
            CypherValue::Node { name, .. } => name.clone(),
            CypherValue::List(items) => {
                let rendered: Vec<String> =
                    items.iter().map(CypherValue::to_display_string).collect();
                format!("[{}]", rendered.join(", "))
            }
            CypherValue::Map(entries) => {
                let rendered: Vec<String> = entries
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.to_display_string()))
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
        }
    }

    /// Renders the value as a JSON fragment, appending to `out`.
    pub fn write_json(&self, out: &mut String) {
        match self {
            CypherValue::Null => out.push_str("null"),
            CypherValue::Bool(b) => {
                let _ = write!(out, "{b}");
            }
            CypherValue::Int(i) => {
                let _ = write!(out, "{i}");
            }
            CypherValue::Str(s) => write_json_string(out, s),
            CypherValue::Node { label, name, .. } => {
                out.push_str("{\"label\":");
                write_json_string(out, label);
                out.push_str(",\"name\":");
                write_json_string(out, name);
                out.push('}');
            }
            CypherValue::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            CypherValue::Map(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_json_string(out, key);
                    out.push(':');
                    value.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

/// Escapes and quotes a string as a JSON string literal.
pub fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
