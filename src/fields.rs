//! `fields`: the caller names the fields it wants in each item of a list, search or
//! transcript, and everything else in those items is left out (#105). A session row is
//! about 450 characters and a search hit more; an agent that wants titles and match
//! indexes pays for all of it otherwise.
//!
//! Only items are pruned. Envelope fields, the ones that say how to read a result
//! (`next_offset`, `truncated_by`, `exhaustive`, `absence_is_proof`, `hidden`, `note`,
//! `revision`, ...), always stay, and so does each item's identity: `provider` and
//! `session_id` on rows and hits, `index` on matches and records.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::schema::property_names;

/// What a result holds items of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    List,
    Search,
    Transcript,
}

impl Shape {
    fn item_names(self) -> Vec<String> {
        match self {
            Shape::List => property_names::<crate::sessions::SessionRow>(),
            Shape::Search => property_names::<crate::sessions::SearchHit>(),
            Shape::Transcript => property_names::<crate::transcript::Record>(),
        }
    }

    /// Item fields kept whatever the caller asks for.
    fn identity(self) -> &'static [&'static str] {
        match self {
            Shape::List | Shape::Search => &["provider", "session_id"],
            Shape::Transcript => &["index"],
        }
    }

    /// Where the items are in a serialized result.
    fn items(self, result: &mut Value) -> Option<&mut Vec<Value>> {
        let v = match self {
            Shape::List => result.get_mut("sessions"),
            Shape::Search => result.get_mut("hits"),
            // `TranscriptResult` flattens its page, so records sit at the top level.
            Shape::Transcript => result.get_mut("records"),
        }?;
        v.as_array_mut()
    }
}

/// Arrays inside an item whose elements can be selected with `name.field`.
const NESTED: &[(&str, &str)] = &[("matches", "index")];

/// A validated selection.
#[derive(Clone, Debug)]
pub struct Fields {
    shape: Shape,
    keep: BTreeSet<String>,
    /// `matches.excerpt` and the like: the array's name, and the fields kept in each element.
    nested: BTreeMap<String, BTreeSet<String>>,
}

impl Fields {
    /// The selection `names` asks for, or None when it names nothing. Unknown names are an
    /// error listing the valid ones: a typo would otherwise return less without saying so.
    pub fn parse(shape: Shape, names: &[String]) -> Result<Option<Self>, String> {
        let names: Vec<&str> = names
            .iter()
            .flat_map(|n| n.split(','))
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .collect();
        if names.is_empty() {
            return Ok(None);
        }
        let valid = shape.item_names();
        let mut keep = BTreeSet::new();
        let mut nested: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut unknown = Vec::new();
        for name in names {
            match name.split_once('.') {
                Some((array, field)) if shape == Shape::Search && is_nested(array) => {
                    if nested_names(array).iter().any(|n| n == field) {
                        nested.entry(array.into()).or_default().insert(field.into());
                    } else {
                        unknown.push(name);
                    }
                }
                None if valid.iter().any(|v| v == name) => {
                    keep.insert(name.to_string());
                }
                _ => unknown.push(name),
            }
        }
        if !unknown.is_empty() {
            let mut all = valid;
            if shape == Shape::Search {
                for (array, _) in NESTED {
                    all.extend(nested_names(array).iter().map(|f| format!("{array}.{f}")));
                }
            }
            return Err(format!(
                "unknown field {}; valid fields: {}",
                unknown.join(", "),
                all.join(", ")
            ));
        }
        Ok(Some(Self {
            shape,
            keep,
            nested,
        }))
    }

    /// Prunes the items of a serialized result in place.
    pub fn apply(&self, result: &mut Value) {
        if let Some(items) = self.shape.items(result) {
            items.iter_mut().for_each(|item| self.prune(item));
        }
    }

    /// The length of `item` as compact JSON after selection, for the size budget.
    pub fn item_chars(&self, item: &impl Serialize) -> usize {
        let Ok(mut v) = serde_json::to_value(item) else {
            return 0;
        };
        self.prune(&mut v);
        serde_json::to_string(&v).map(|s| s.len()).unwrap_or(0)
    }

    fn prune(&self, item: &mut Value) {
        let Some(o) = item.as_object_mut() else {
            return;
        };
        let identity = self.shape.identity();
        o.retain(|k, _| {
            identity.contains(&k.as_str()) || self.keep.contains(k) || self.nested.contains_key(k)
        });
        for (array, fields) in &self.nested {
            // A plain `matches` keeps every field of every match.
            if self.keep.contains(array) {
                continue;
            }
            let id = NESTED
                .iter()
                .find(|(a, _)| a == array)
                .map(|(_, id)| *id)
                .unwrap_or_default();
            if let Some(Value::Array(elements)) = o.get_mut(array) {
                for e in elements.iter_mut().filter_map(Value::as_object_mut) {
                    e.retain(|k, _| k == id || fields.contains(k));
                }
            }
        }
    }
}

fn is_nested(array: &str) -> bool {
    NESTED.iter().any(|(a, _)| *a == array)
}

fn nested_names(array: &str) -> Vec<String> {
    match array {
        "matches" => property_names::<crate::index::Match>(),
        _ => Vec::new(),
    }
}

/// Selects `names` in a serialized result of `shape`. The one function both doors call
/// before printing, so the CLI and the MCP server return the same JSON.
pub fn select(result: &mut Value, shape: Shape, names: &[String]) -> Result<(), String> {
    if let Some(f) = Fields::parse(shape, names)? {
        f.apply(result);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_search_hit_keeps_its_identity_and_the_named_fields() {
        let mut r = json!({"next_offset": 3, "hits": [{
            "provider": "claude", "session_id": "a", "title": "t", "snippet": "s",
            "match_count": 2, "matches": [{"index": 4, "type": "user", "excerpt": "e"}]}]});
        select(&mut r, Shape::Search, &names(&["title", "matches.excerpt"])).unwrap();
        assert_eq!(
            r,
            json!({"next_offset": 3, "hits": [{"provider": "claude", "session_id": "a",
                "title": "t", "matches": [{"index": 4, "excerpt": "e"}]}]})
        );
    }

    #[test]
    fn comma_separated_names_are_split() {
        let f = Fields::parse(Shape::List, &names(&["title,modified", " snippet "]))
            .unwrap()
            .unwrap();
        assert_eq!(f.keep.len(), 3);
    }

    #[test]
    fn an_unknown_name_is_an_error_listing_the_valid_ones() {
        let e = Fields::parse(Shape::Search, &names(&["titel", "matches.nope"])).unwrap_err();
        assert!(
            e.starts_with("unknown field titel, matches.nope; valid fields: "),
            "{e}"
        );
        assert!(e.contains("matches.excerpt"), "{e}");
        assert!(Fields::parse(Shape::List, &names(&["matches.index"])).is_err());
    }

    #[test]
    fn no_names_selects_nothing() {
        assert!(Fields::parse(Shape::List, &[]).unwrap().is_none());
        assert!(
            Fields::parse(Shape::List, &names(&[" , "]))
                .unwrap()
                .is_none()
        );
    }
}
