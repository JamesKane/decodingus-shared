//! Phylogenetic tree merge — the "Identify-Match-Graft" algorithm, re-implemented.
//!
//! Given an incoming **source** tree (e.g. ISOGG / ytree.net) and the existing
//! **production** tree, produce a [`MergePlan`]: a list of [`MergeOp`]s (create /
//! reparent / variant edits) plus [`Ambiguity`] flags for cases a human must
//! resolve. This is a *pure* function — no IO; `du-db` materializes the plan
//! into a change set, and curators review it before it touches production.
//!
//! The legacy implementation was buggy (notably "premature branch creation" via
//! over-eager global variant matching), so this is a re-implementation driven by
//! curated fixtures, **not** a golden-test port. The design choice that keeps it
//! honest is to be *conservative*: handle the unambiguous cases precisely and
//! flag everything else rather than guess.
//!
//! ## How matching works
//!
//! Each node carries its **defining** variant set U(N) — the SNPs that branch at
//! that node. A source node `S` (with defining set `VS`) is matched against a
//! *scope* of existing nodes (the descendants of wherever `S`'s parent matched —
//! this subtree scoping is the recurrent-SNP guard: an L21 recurring in an
//! unrelated lineage is simply out of scope, so it can't cause a cross-graft).
//!
//! Within scope, with the single overlapping candidate `E` (set `UE`):
//! - `VS == UE` → **FULL_MATCH**: same node; merge attribution, recurse.
//! - `VS ⊂ UE` → **CONTRACTION**: the source splits a coarser existing node.
//!   Create `S` as an intermediate, reparent `E` beneath it, and *downflow* the
//!   shared variants off `E` (they now live on the new ancestor).
//! - `UE ⊂ VS` → **DESCENDANT**: the source is finer; attach a new child under `E`.
//! - no overlap in scope → **NEW**: create the node (and its subtree).
//! - partial overlap / multiple candidates → **AMBIGUITY**: flag, and still
//!   create the node so the import isn't lost, but leave placement to a curator.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// An incoming source-tree node. `variants` are the SNP names that define this
/// node in the source nomenclature (may be empty for an unnamed intermediate).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceNode {
    pub name: String,
    #[serde(default)]
    pub variants: Vec<String>,
    #[serde(default)]
    pub children: Vec<SourceNode>,
}

/// An existing production-tree node. `variants` is U(N): the defining SNPs at
/// this node only.
#[derive(Debug, Clone)]
pub struct ExistingNode {
    pub id: i64,
    pub name: String,
    pub variants: Vec<String>,
    pub children: Vec<ExistingNode>,
}

/// Where an op attaches: an existing production id, a placeholder for a node
/// created earlier in this same plan, or a new root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum NodeRef {
    Root,
    Existing(i64),
    New(i32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MergeOp {
    /// Create a new node (negative `placeholder` id) under `parent`.
    CreateNode {
        placeholder: i32,
        name: String,
        parent: NodeRef,
        variants: Vec<String>,
    },
    /// Reparent an existing node (node contraction).
    Reparent { node: i64, new_parent: NodeRef },
    /// Edit an existing node's defining variants (downflow uses `remove`).
    EditVariants {
        node: i64,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    /// The source matched an existing node 1:1 — record source attribution.
    MatchMetadata { node: i64, source: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguityKind {
    /// Variants overlap but neither set contains the other.
    PartialMatch,
    /// More than one in-scope node shares variants with the source node.
    MultipleCandidates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ambiguity {
    pub source_name: String,
    pub kind: AmbiguityKind,
    pub detail: String,
    pub candidates: Vec<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeStats {
    pub processed: usize,
    pub matched: usize,
    pub created: usize,
    pub contracted: usize,
    pub ambiguous: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MergePlan {
    pub ops: Vec<MergeOp>,
    pub ambiguities: Vec<Ambiguity>,
    pub stats: MergeStats,
}

// ── internal index over the existing tree ─────────────────────────────────────

struct Index<'a> {
    by_id: BTreeMap<i64, &'a ExistingNode>,
    /// U(N) as a set, per node.
    defining: BTreeMap<i64, BTreeSet<String>>,
    /// All descendant ids of N, *including* N.
    subtree: BTreeMap<i64, BTreeSet<i64>>,
}

impl<'a> Index<'a> {
    fn build(roots: &'a [ExistingNode]) -> Self {
        let mut idx = Index { by_id: BTreeMap::new(), defining: BTreeMap::new(), subtree: BTreeMap::new() };
        for r in roots {
            idx.walk(r);
        }
        idx
    }

    /// Populate maps; returns the subtree-id set of `node` (incl. self).
    fn walk(&mut self, node: &'a ExistingNode) -> BTreeSet<i64> {
        self.by_id.insert(node.id, node);
        self.defining.insert(node.id, node.variants.iter().cloned().collect());
        let mut sub = BTreeSet::new();
        sub.insert(node.id);
        for c in &node.children {
            let cs = self.walk(c);
            sub.extend(cs);
        }
        self.subtree.insert(node.id, sub.clone());
        sub
    }

    fn all_ids(&self) -> BTreeSet<i64> {
        self.by_id.keys().copied().collect()
    }

    /// Strict descendants of `id` (excludes `id` itself).
    fn strict_descendants(&self, id: i64) -> BTreeSet<i64> {
        let mut s = self.subtree.get(&id).cloned().unwrap_or_default();
        s.remove(&id);
        s
    }
}

enum MatchResult {
    Full(i64),
    Contraction(i64),
    Descendant(i64),
    New,
    Ambiguous(AmbiguityKind, Vec<i64>),
}

/// Classify a source node (defining set `vs`, `name`) against an in-scope set of
/// existing node ids.
fn classify(vs: &BTreeSet<String>, name: &str, scope: &BTreeSet<i64>, idx: &Index) -> MatchResult {
    // Unnamed/variant-less intermediate: fall back to a name match in scope.
    if vs.is_empty() {
        let by_name: Vec<i64> = scope.iter().copied().filter(|id| idx.by_id[id].name == name).collect();
        return if by_name.len() == 1 { MatchResult::Full(by_name[0]) } else { MatchResult::New };
    }

    let candidates: Vec<i64> = scope
        .iter()
        .copied()
        .filter(|id| !idx.defining[id].is_disjoint(vs))
        .collect();

    let exact: Vec<i64> = candidates.iter().copied().filter(|id| idx.defining[id] == *vs).collect();
    match exact.len() {
        1 => return MatchResult::Full(exact[0]),
        n if n > 1 => return MatchResult::Ambiguous(AmbiguityKind::MultipleCandidates, exact),
        _ => {}
    }

    match candidates.len() {
        0 => MatchResult::New,
        1 => {
            let e = candidates[0];
            let ue = &idx.defining[&e];
            if vs.is_subset(ue) {
                MatchResult::Contraction(e)
            } else if ue.is_subset(vs) {
                MatchResult::Descendant(e)
            } else {
                MatchResult::Ambiguous(AmbiguityKind::PartialMatch, candidates)
            }
        }
        _ => MatchResult::Ambiguous(AmbiguityKind::MultipleCandidates, candidates),
    }
}

struct Planner<'a> {
    idx: Index<'a>,
    plan: MergePlan,
    next_placeholder: i32,
}

impl<'a> Planner<'a> {
    fn new(idx: Index<'a>) -> Self {
        Planner { idx, plan: MergePlan::default(), next_placeholder: -1 }
    }

    fn placeholder(&mut self) -> i32 {
        let p = self.next_placeholder;
        self.next_placeholder -= 1;
        p
    }

    /// Create a node under `parent`, returning its placeholder ref.
    fn create(&mut self, name: &str, parent: NodeRef, variants: Vec<String>) -> NodeRef {
        let placeholder = self.placeholder();
        self.plan.ops.push(MergeOp::CreateNode { placeholder, name: name.to_string(), parent, variants });
        self.plan.stats.created += 1;
        NodeRef::New(placeholder)
    }

    fn visit(&mut self, source: &SourceNode, parent: NodeRef, scope: BTreeSet<i64>, source_name: &str) {
        self.plan.stats.processed += 1;
        let vs: BTreeSet<String> = source.variants.iter().cloned().collect();

        match classify(&vs, &source.name, &scope, &self.idx) {
            MatchResult::Full(e) => {
                self.plan.ops.push(MergeOp::MatchMetadata { node: e, source: source_name.to_string() });
                self.plan.stats.matched += 1;
                let child_scope = self.idx.strict_descendants(e);
                for c in &source.children {
                    self.visit(c, NodeRef::Existing(e), child_scope.clone(), source_name);
                }
            }
            MatchResult::Contraction(e) => {
                // Source is a coarser ancestor of E: insert it above E, move E
                // under it, and downflow the shared variants off E.
                let new_ref = self.create(&source.name, parent, source.variants.clone());
                self.plan.ops.push(MergeOp::Reparent { node: e, new_parent: new_ref });
                let shared: Vec<String> = vs.iter().cloned().collect();
                if !shared.is_empty() {
                    self.plan.ops.push(MergeOp::EditVariants { node: e, add: vec![], remove: shared });
                }
                self.plan.stats.contracted += 1;
                // E now sits under the new node; deeper source nodes match E or
                // its descendants.
                let mut child_scope = self.idx.strict_descendants(e);
                child_scope.insert(e);
                for c in &source.children {
                    self.visit(c, new_ref, child_scope.clone(), source_name);
                }
            }
            MatchResult::Descendant(e) => {
                // Source is finer than E: attach a new child carrying the extra
                // SNPs. (Conservative: we do not pull E's existing children into
                // the new node — that restructuring is left to a curator.)
                let ue = &self.idx.defining[&e];
                let extra: Vec<String> = source.variants.iter().filter(|v| !ue.contains(*v)).cloned().collect();
                let new_ref = self.create(&source.name, NodeRef::Existing(e), extra);
                for c in &source.children {
                    self.visit(c, new_ref, BTreeSet::new(), source_name);
                }
            }
            MatchResult::New => {
                let new_ref = self.create(&source.name, parent, source.variants.clone());
                for c in &source.children {
                    self.visit(c, new_ref, BTreeSet::new(), source_name);
                }
            }
            MatchResult::Ambiguous(kind, candidates) => {
                self.plan.ambiguities.push(Ambiguity {
                    source_name: source.name.clone(),
                    kind,
                    detail: format!("'{}' could not be placed unambiguously", source.name),
                    candidates,
                });
                self.plan.stats.ambiguous += 1;
                // Preserve the import: create it under the parent for a curator
                // to relocate, and keep walking its subtree as new.
                let new_ref = self.create(&source.name, parent, source.variants.clone());
                for c in &source.children {
                    self.visit(c, new_ref, BTreeSet::new(), source_name);
                }
            }
        }
    }
}

/// Merge a source tree into the existing tree, producing a reviewable plan.
pub fn merge(existing_roots: &[ExistingNode], source_roots: &[SourceNode], source_name: &str) -> MergePlan {
    let idx = Index::build(existing_roots);
    let all = idx.all_ids();
    let mut planner = Planner::new(idx);
    for root in source_roots {
        planner.visit(root, NodeRef::Root, all.clone(), source_name);
    }
    planner.plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(id: i64, name: &str, vars: &[&str], children: Vec<ExistingNode>) -> ExistingNode {
        ExistingNode { id, name: name.into(), variants: vars.iter().map(|s| s.to_string()).collect(), children }
    }
    fn src(name: &str, vars: &[&str], children: Vec<SourceNode>) -> SourceNode {
        SourceNode { name: name.into(), variants: vars.iter().map(|s| s.to_string()).collect(), children }
    }
    fn creates(p: &MergePlan) -> Vec<&str> {
        p.ops.iter().filter_map(|o| match o {
            MergeOp::CreateNode { name, .. } => Some(name.as_str()),
            _ => None,
        }).collect()
    }

    #[test]
    fn identical_tree_is_a_noop() {
        // existing R -> R1b ; source identical.
        let existing = vec![ex(1, "R", &["M207"], vec![ex(2, "R1b", &["M343"], vec![])])];
        let source = vec![src("R", &["M207"], vec![src("R1b", &["M343"], vec![])])];
        let plan = merge(&existing, &source, "ISOGG");
        assert!(creates(&plan).is_empty(), "no new nodes for an identical tree");
        assert!(plan.ambiguities.is_empty());
        assert_eq!(plan.stats.matched, 2);
        // both nodes matched 1:1
        assert_eq!(plan.ops.iter().filter(|o| matches!(o, MergeOp::MatchMetadata { .. })).count(), 2);
    }

    #[test]
    fn pure_extension_adds_a_leaf() {
        // source adds R1b-L21 under the existing R1b.
        let existing = vec![ex(1, "R", &["M207"], vec![ex(2, "R1b", &["M343"], vec![])])];
        let source = vec![src("R", &["M207"], vec![src("R1b", &["M343"], vec![src("R1b-L21", &["L21"], vec![])])])];
        let plan = merge(&existing, &source, "ISOGG");
        assert_eq!(creates(&plan), vec!["R1b-L21"]);
        // attached under the existing R1b (id 2)
        match plan.ops.iter().find(|o| matches!(o, MergeOp::CreateNode { .. })).unwrap() {
            MergeOp::CreateNode { parent, variants, .. } => {
                assert_eq!(*parent, NodeRef::Existing(2));
                assert_eq!(variants, &vec!["L21".to_string()]);
            }
            _ => unreachable!(),
        }
        assert!(plan.ambiguities.is_empty());
    }

    #[test]
    fn node_contraction_splits_a_coarse_node() {
        // Existing lumps three SNPs on one node; source splits the top one out.
        //   existing:  R(M207) -> RC(M343,L23,L51)
        //   source:    R(M207) -> R1b(M343) -> [deeper handled separately]
        let existing = vec![ex(1, "R", &["M207"], vec![ex(2, "RC", &["M343", "L23", "L51"], vec![])])];
        let source = vec![src("R", &["M207"], vec![src("R1b", &["M343"], vec![])])];
        let plan = merge(&existing, &source, "ISOGG");

        // R matches; R1b (subset of RC's defining set) triggers contraction.
        assert_eq!(creates(&plan), vec!["R1b"]);
        assert_eq!(plan.stats.contracted, 1);
        // New node attaches under R (id 1); RC (id 2) reparented under the new
        // node; M343 downflowed off RC.
        let create = plan.ops.iter().find_map(|o| match o {
            MergeOp::CreateNode { placeholder, parent, .. } => Some((*placeholder, *parent)),
            _ => None,
        }).unwrap();
        assert_eq!(create.1, NodeRef::Existing(1));
        assert!(plan.ops.iter().any(|o| matches!(o, MergeOp::Reparent { node: 2, new_parent } if *new_parent == NodeRef::New(create.0))));
        assert!(plan.ops.iter().any(|o| matches!(o, MergeOp::EditVariants { node: 2, remove, .. } if remove == &vec!["M343".to_string()])));
        assert!(plan.ambiguities.is_empty());
    }

    #[test]
    fn recurrent_snp_does_not_cross_graft() {
        // L21 defines R1b-L21, but the SAME name recurs on an unrelated I node.
        // Subtree scoping must keep the source's R-lineage L21 from matching the
        // I-lineage node: we get a NEW node under R1b, NOT a reparent of I-L21.
        let existing = vec![
            ex(1, "R", &["M207"], vec![ex(2, "R1b", &["M343"], vec![])]),
            ex(3, "I", &["M170"], vec![ex(4, "I-L21", &["L21"], vec![])]),
        ];
        let source = vec![src("R", &["M207"], vec![src("R1b", &["M343"], vec![src("R-L21", &["L21"], vec![])])])];
        let plan = merge(&existing, &source, "ISOGG");
        assert_eq!(creates(&plan), vec!["R-L21"]);
        // created under R1b (id 2), and the I-lineage node (id 4) is untouched.
        assert!(plan.ops.iter().any(|o| matches!(o, MergeOp::CreateNode { parent, .. } if *parent == NodeRef::Existing(2))));
        assert!(!plan.ops.iter().any(|o| matches!(o, MergeOp::Reparent { node: 4, .. })));
        assert!(plan.ambiguities.is_empty(), "scoping resolves it cleanly, no ambiguity");
    }

    #[test]
    fn partial_overlap_is_flagged_not_guessed() {
        // Source node shares one SNP with an existing node but adds a conflicting
        // one and is missing another — neither set contains the other.
        let existing = vec![ex(1, "R", &["M207"], vec![ex(2, "X", &["A", "B"], vec![])])];
        let source = vec![src("R", &["M207"], vec![src("Y", &["A", "C"], vec![])])];
        let plan = merge(&existing, &source, "ISOGG");
        assert_eq!(plan.stats.ambiguous, 1);
        assert_eq!(plan.ambiguities[0].kind, AmbiguityKind::PartialMatch);
        assert_eq!(plan.ambiguities[0].candidates, vec![2]);
        // import preserved as a new node under R for the curator to relocate
        assert_eq!(creates(&plan), vec!["Y"]);
    }

    #[test]
    fn new_subtree_chains_placeholders() {
        // Source adds a two-deep new subtree; the deeper node attaches to the
        // shallower one's placeholder.
        let existing = vec![ex(1, "R", &["M207"], vec![])];
        let source = vec![src("R", &["M207"], vec![src("A", &["a1"], vec![src("B", &["b1"], vec![])])])];
        let plan = merge(&existing, &source, "ISOGG");
        assert_eq!(creates(&plan), vec!["A", "B"]);
        let a_ph = plan.ops.iter().find_map(|o| match o {
            MergeOp::CreateNode { placeholder, name, parent, .. } if name == "A" => {
                assert_eq!(*parent, NodeRef::Existing(1));
                Some(*placeholder)
            }
            _ => None,
        }).unwrap();
        // B attaches under A's placeholder.
        assert!(plan.ops.iter().any(|o| matches!(o, MergeOp::CreateNode { name, parent, .. } if name == "B" && *parent == NodeRef::New(a_ph))));
    }
}
