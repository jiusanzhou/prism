//! P/D (Prefill/Decode) Routing Plugin
//!
//! Handles disaggregated inference where prefill and decode run on separate instances.
//!
//! ## Instance types
//!
//! - **Colocated** (no role label): single instance does both P and D → route normally
//! - **Prefill** (role=prefill): only handles first pass (prompt encoding)
//! - **Decode** (role=decode): only handles autoregressive generation
//!
//! ## Routing rules
//!
//! For colocated variants: normal single-variant routing (no P/D logic).
//!
//! For P/D split variants:
//! 1. Filter candidates by pd_group completeness (need both P and D in same group)
//! 2. Select a P variant (using standard scoring)
//! 3. Find matching D variant from same pd_group
//! 4. Return both as PDDecision

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// P/D role extracted from variant metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PDRole {
    Colocated,
    Prefill,
    Decode,
}

impl PDRole {
    pub fn from_metadata(metadata: &HashMap<String, String>) -> Self {
        match metadata.get(META_ROLE).map(|s| s.as_str()) {
            Some("prefill") => PDRole::Prefill,
            Some("decode") => PDRole::Decode,
            _ => PDRole::Colocated,
        }
    }
}

/// A P/D routing decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PDDecision {
    /// Colocated — single variant handles both.
    Single { variant_id: String },
    /// P/D split — two variants in the same group.
    Split {
        prefill_variant_id: String,
        decode_variant_id: String,
        pd_group: String,
    },
}

pub const META_ROLE: &str = "wanqing.inference.instance/role";
pub const META_PD_GROUP: &str = "wanqing.inference.instance.pd/group";
pub const META_LEADER_ID: &str = "apps.kwai.io/duplicateset-instance-id";
pub const LEADER_VALUE: &str = "0";

pub fn is_leader(metadata: &HashMap<String, String>) -> bool {
    metadata.get(META_LEADER_ID).map(|v| v.as_str()) == Some(LEADER_VALUE)
}

/// Group P/D variants and validate completeness.
pub fn group_pd_variants(
    variant_metadata: &[(String, HashMap<String, String>)],
) -> PDGroupResult {
    let mut colocated = Vec::new();
    let mut groups: HashMap<String, PDGroup> = HashMap::new();

    for (variant_id, meta) in variant_metadata {
        let role = PDRole::from_metadata(meta);
        match role {
            PDRole::Colocated => colocated.push(variant_id.clone()),
            PDRole::Prefill | PDRole::Decode => {
                let pd_group = meta.get(META_PD_GROUP).cloned().unwrap_or("default".into());
                let group = groups.entry(pd_group).or_default();
                if is_leader(meta) {
                    match role {
                        PDRole::Prefill => group.prefill_leaders.push(variant_id.clone()),
                        PDRole::Decode => group.decode_leaders.push(variant_id.clone()),
                        _ => {}
                    }
                }
            }
        }
    }

    let complete_groups = groups
        .into_iter()
        .filter(|(_, g)| !g.prefill_leaders.is_empty() && !g.decode_leaders.is_empty())
        .collect();

    PDGroupResult { colocated, pd_groups: complete_groups }
}

#[derive(Debug, Default)]
pub struct PDGroupResult {
    pub colocated: Vec<String>,
    pub pd_groups: HashMap<String, PDGroup>,
}

impl PDGroupResult {
    /// All valid variant IDs (colocated + leaders from complete groups).
    pub fn all_valid_variant_ids(&self) -> Vec<String> {
        let mut ids = self.colocated.clone();
        for group in self.pd_groups.values() {
            ids.extend(group.prefill_leaders.clone());
            ids.extend(group.decode_leaders.clone());
        }
        ids
    }

    /// Resolve a selected variant into a P/D decision.
    pub fn resolve(
        &self,
        selected_variant_id: &str,
        all_metadata: &HashMap<String, HashMap<String, String>>,
    ) -> PDDecision {
        if self.colocated.contains(&selected_variant_id.to_string()) {
            return PDDecision::Single { variant_id: selected_variant_id.into() };
        }

        let meta = match all_metadata.get(selected_variant_id) {
            Some(m) => m,
            None => return PDDecision::Single { variant_id: selected_variant_id.into() },
        };

        let role = PDRole::from_metadata(meta);
        let pd_group = meta.get(META_PD_GROUP).cloned().unwrap_or("default".into());

        let group = match self.pd_groups.get(&pd_group) {
            Some(g) => g,
            None => return PDDecision::Single { variant_id: selected_variant_id.into() },
        };

        match role {
            PDRole::Prefill => PDDecision::Split {
                prefill_variant_id: selected_variant_id.into(),
                decode_variant_id: group.decode_leaders.first().cloned().unwrap_or_default(),
                pd_group,
            },
            PDRole::Decode => PDDecision::Split {
                prefill_variant_id: group.prefill_leaders.first().cloned().unwrap_or_default(),
                decode_variant_id: selected_variant_id.into(),
                pd_group,
            },
            PDRole::Colocated => PDDecision::Single { variant_id: selected_variant_id.into() },
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct PDGroup {
    pub prefill_leaders: Vec<String>,
    pub decode_leaders: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(role: &str, group: &str, leader: bool) -> HashMap<String, String> {
        let mut m = HashMap::new();
        if !role.is_empty() { m.insert(META_ROLE.into(), role.into()); }
        if !group.is_empty() { m.insert(META_PD_GROUP.into(), group.into()); }
        if leader { m.insert(META_LEADER_ID.into(), LEADER_VALUE.into()); }
        m
    }

    #[test]
    fn test_colocated_only() {
        let v = vec![("v1".into(), HashMap::new()), ("v2".into(), HashMap::new())];
        let r = group_pd_variants(&v);
        assert_eq!(r.colocated.len(), 2);
        assert!(r.pd_groups.is_empty());
    }

    #[test]
    fn test_complete_pd_group() {
        let v = vec![
            ("p1".into(), meta("prefill", "g1", true)),
            ("d1".into(), meta("decode", "g1", true)),
        ];
        let r = group_pd_variants(&v);
        assert_eq!(r.pd_groups.len(), 1);
        assert_eq!(r.pd_groups["g1"].prefill_leaders, vec!["p1"]);
        assert_eq!(r.pd_groups["g1"].decode_leaders, vec!["d1"]);
    }

    #[test]
    fn test_incomplete_group_filtered() {
        let v = vec![("p1".into(), meta("prefill", "g1", true))];
        let r = group_pd_variants(&v);
        assert!(r.pd_groups.is_empty());
    }

    #[test]
    fn test_resolve_prefill_finds_decode() {
        let v = vec![
            ("p1".into(), meta("prefill", "g1", true)),
            ("d1".into(), meta("decode", "g1", true)),
        ];
        let r = group_pd_variants(&v);
        let all: HashMap<String, HashMap<String, String>> = v.into_iter().collect();
        match r.resolve("p1", &all) {
            PDDecision::Split { prefill_variant_id, decode_variant_id, pd_group } => {
                assert_eq!(prefill_variant_id, "p1");
                assert_eq!(decode_variant_id, "d1");
                assert_eq!(pd_group, "g1");
            }
            _ => panic!("expected Split"),
        }
    }

    #[test]
    fn test_resolve_decode_finds_prefill() {
        let v = vec![
            ("p1".into(), meta("prefill", "g1", true)),
            ("d1".into(), meta("decode", "g1", true)),
        ];
        let r = group_pd_variants(&v);
        let all: HashMap<String, HashMap<String, String>> = v.into_iter().collect();
        match r.resolve("d1", &all) {
            PDDecision::Split { prefill_variant_id, decode_variant_id, .. } => {
                assert_eq!(prefill_variant_id, "p1");
                assert_eq!(decode_variant_id, "d1");
            }
            _ => panic!("expected Split"),
        }
    }

    #[test]
    fn test_non_leader_excluded() {
        let v = vec![
            ("p1".into(), meta("prefill", "g1", false)),
            ("d1".into(), meta("decode", "g1", true)),
        ];
        let r = group_pd_variants(&v);
        assert!(r.pd_groups.is_empty());
    }

    #[test]
    fn test_multiple_groups() {
        let v = vec![
            ("p1".into(), meta("prefill", "g1", true)),
            ("d1".into(), meta("decode", "g1", true)),
            ("p2".into(), meta("prefill", "g2", true)),
            ("d2".into(), meta("decode", "g2", true)),
        ];
        let r = group_pd_variants(&v);
        assert_eq!(r.pd_groups.len(), 2);
        assert_eq!(r.all_valid_variant_ids().len(), 4);
    }

    #[test]
    fn test_mixed_colocated_and_pd() {
        let v = vec![
            ("co1".into(), HashMap::new()),
            ("p1".into(), meta("prefill", "g1", true)),
            ("d1".into(), meta("decode", "g1", true)),
        ];
        let r = group_pd_variants(&v);
        assert_eq!(r.colocated, vec!["co1"]);
        assert_eq!(r.pd_groups.len(), 1);
        assert_eq!(r.all_valid_variant_ids().len(), 3);
    }
}
