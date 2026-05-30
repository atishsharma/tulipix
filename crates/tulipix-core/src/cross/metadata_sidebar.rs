//! `np.p4.metadata-sidebar` — single right-rail metadata component.
//!
//! Every section needs a metadata panel; rather than five bespoke ones, each
//! section produces a normalized [`MetaField`] list and this renders them
//! uniformly (grouped, ordered, empties hidden). This module owns the field
//! model + the builder that drops empty values and groups fields by section.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Group { General, Technical, Location, People, File }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaField {
    pub group: Group,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Default, Clone)]
pub struct PanelBuilder {
    fields: Vec<MetaField>,
}

impl PanelBuilder {
    pub fn new() -> Self { Self::default() }

    /// Add a field; silently skips `None`/empty so callers don't branch.
    pub fn add(mut self, group: Group, label: &str, value: Option<&str>) -> Self {
        if let Some(v) = value {
            let v = v.trim();
            if !v.is_empty() { self.fields.push(MetaField { group, label: label.to_string(), value: v.to_string() }); }
        }
        self
    }

    /// Final field list ordered by group (General→Technical→Location→People→
    /// File), preserving insertion order within a group.
    pub fn build(self) -> Vec<MetaField> {
        fn rank(g: Group) -> u8 {
            match g { Group::General => 0, Group::Technical => 1, Group::Location => 2, Group::People => 3, Group::File => 4 }
        }
        let mut f = self.fields;
        f.sort_by_key(|m| rank(m.group)); // stable: keeps within-group order
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_empty_and_orders_by_group() {
        let fields = PanelBuilder::new()
            .add(Group::File, "Path", Some("/a/b.jpg"))
            .add(Group::General, "Title", Some("Sunset"))
            .add(Group::Technical, "Codec", Some(" "))     // blank → skipped
            .add(Group::Location, "GPS", None)              // none → skipped
            .add(Group::People, "Faces", Some("Alice, Bob"))
            .build();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].group, Group::General); // General before File
        assert_eq!(fields[0].label, "Title");
        assert_eq!(fields.last().unwrap().group, Group::File);
    }
}
