//! Explicit operation-DAG semantics for plans (V6-F6 / ZS-EXEC-001).
//!
//! A plan is not an opaque linear list: every step is a node, every declared
//! `needs` entry is a data-dependency edge (output of the producer feeds the
//! dependent), cycles are rejected fail-loud with the offending path, and the
//! derived topological schedule groups steps into batch-parallel waves
//! (independent steps share a wave and are therefore declared parallelizable).
//!
//! Execution remains sequential -- the schedule ORDER honors the DAG, which is
//! exactly what makes forward-declared dependencies legal. The DAG structure
//! (nodes, edges, levels) is part of the plan receipt, not a private
//! implementation detail.

use super::types::{PlanStep, Program};
use serde_json::{json, Value};
use std::collections::HashMap;

/// A single data-dependency edge: `from` (producer node id) feeds `to`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DagEdge {
    pub from: String,
    pub to: String,
}

/// Derived plan DAG: nodes (step-level ids), edges (declared data deps
/// resolved to step-level nodes), and the topological schedule.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlanDag {
    /// Step-level node ids in declaration order (step `id`, or `step{i}`).
    pub nodes: Vec<String>,
    /// Declared data dependencies, resolved to step-level node ids.
    /// Parallel branch ids resolve to their owning group's node.
    pub edges: Vec<DagEdge>,
    /// Batch-parallel groups: one topological wave per entry; every node in a
    /// wave has all its producers in earlier waves, so the whole wave is
    /// parallelizable (declared independent).
    pub levels: Vec<Vec<String>>,
    /// Topological execution order (node ids, level-major).
    pub schedule: Vec<String>,
    /// Step list indices in topological order (execution schedule).
    pub schedule_indices: Vec<usize>,
}

impl PlanDag {
    /// Build the DAG for a parsed program. Fails loudly (never silently) on:
    /// - a `needs` entry that names no declared id (step or branch id), and
    /// - any dependency cycle (including self-dependency), naming the path.
    pub fn build(program: &Program) -> Result<Self, String> {
        // Step-level nodes in declaration order.
        let nodes: Vec<String> = program
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| step_node_id(step, i))
            .collect();
        // Branch ids resolve to their owning step's node; step ids resolve to
        // themselves. Duplicate ids are already rejected by validate_program,
        // so collisions here are internal errors, not plan errors.
        let mut resolve: HashMap<&str, usize> = HashMap::new();
        for (i, step) in program.steps.iter().enumerate() {
            resolve.insert(nodes[i].as_str(), i);
            match step {
                PlanStep::Parallel { branches, .. } => {
                    for branch in branches {
                        if resolve.insert(branch.id.as_str(), i).is_some() {
                            return Err(format!(
                                "plan: internal duplicate id resolution for '{}'",
                                branch.id
                            ));
                        }
                    }
                }
                PlanStep::Call { .. } => {}
            }
        }

        // Edges: declared needs -> step-level nodes.
        let mut edges: Vec<DagEdge> = Vec::new();
        for (i, step) in program.steps.iter().enumerate() {
            for need in step.needs() {
                let Some(&producer) = resolve.get(need.as_str()) else {
                    return Err(format!("step {i}: unmet dependency '{need}'"));
                };
                edges.push(DagEdge {
                    from: nodes[producer].clone(),
                    to: nodes[i].clone(),
                });
            }
        }

        // Kahn's algorithm: waves of indegree-zero nodes, declaration order
        // preserved within a wave.
        let mut indegree: HashMap<&str, usize> =
            nodes.iter().map(|id| (id.as_str(), 0usize)).collect();
        let mut successors: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &edges {
            indegree.insert(
                edge.to.as_str(),
                indegree.get(edge.to.as_str()).copied().unwrap_or(0) + 1,
            );
            successors
                .entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }
        let mut remaining: Vec<&str> = nodes.iter().map(String::as_str).collect();
        let mut levels: Vec<Vec<String>> = Vec::new();
        let mut schedule: Vec<String> = Vec::new();
        while !remaining.is_empty() {
            let mut wave: Vec<&str> = Vec::new();
            for id in &remaining {
                if indegree.get(id).copied().unwrap_or(0) == 0 {
                    wave.push(id);
                }
            }
            if wave.is_empty() {
                let path = find_cycle(&remaining, &successors);
                return Err(format!(
                    "plan: cycle rejected: {}",
                    path.join(" -> ")
                ));
            }
            for id in &wave {
                if let Some(succs) = successors.get(id) {
                    for succ in succs {
                        indegree.insert(
                            succ,
                            indegree.get(succ).copied().unwrap_or(1).saturating_sub(1),
                        );
                    }
                }
            }
            let wave_owned: Vec<String> = wave.iter().map(|s| s.to_string()).collect();
            levels.push(wave_owned.clone());
            schedule.extend(wave_owned);
            remaining.retain(|id| !wave.contains(id));
        }

        // Map schedule ids back to step list indices (declared order indices;
        // bindings and step logs keep the declared index).
        let index_of: HashMap<&str, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let schedule_indices = schedule
            .iter()
            .map(|id| index_of[id.as_str()])
            .collect::<Vec<_>>();

        Ok(Self {
            nodes,
            edges,
            levels,
            schedule,
            schedule_indices,
        })
    }

    /// Compact receipt JSON: nodes, edges, batch-parallel levels, schedule.
    pub fn to_json(&self) -> Value {
        json!({
            "nodes": self.nodes,
            "edges": self.edges,
            "levels": self.levels,
            "schedule": self.schedule,
        })
    }
}

fn step_node_id(step: &PlanStep, index: usize) -> String {
    match step {
        PlanStep::Call { id, .. } | PlanStep::Parallel { id, .. } => {
            id.clone().unwrap_or_else(|| format!("step{index}"))
        }
    }
}

/// Walk a cycle within `remaining`. Kahn stalls exactly when every remaining
/// node has at least one unscheduled predecessor, so following predecessors
/// backward from any remaining node stays inside `remaining` and must close a
/// cycle. Returns the cycle path starting and ending at the same id.
fn find_cycle<'a>(
    remaining: &[&'a str],
    successors: &HashMap<&'a str, Vec<&'a str>>,
) -> Vec<String> {
    let mut predecessors: HashMap<&'a str, Vec<&'a str>> = HashMap::new();
    for (&from, tos) in successors {
        for to in tos {
            predecessors.entry(to).or_default().push(from);
        }
    }
    for start in remaining {
        let mut path: Vec<&'a str> = Vec::new();
        let mut current = *start;
        loop {
            if let Some(pos) = path.iter().position(|p| *p == current) {
                let mut cycle: Vec<String> = path[pos..]
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>();
                cycle.push(current.to_string());
                return cycle;
            }
            path.push(current);
            let Some(preds) = predecessors.get(current) else {
                break;
            };
            let Some(&next) = preds.iter().find(|p| remaining.contains(p)) else {
                break;
            };
            current = next;
        }
    }
    // Unreachable in practice (Kahn stall implies a cycle); keep a stable
    // diagnostic instead of looping forever.
    vec!["?".to_string(), "?".to_string()]
}
