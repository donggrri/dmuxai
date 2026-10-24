use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::agent::Agent;
use dmuxai::graph::NodeAssignment;

/// Minimum wait between graph defer retries (FR-976); zero in unit tests.
pub(crate) const GRAPH_DEFER_RETRY_INTERVAL: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(1)
};

/// FR-976: if the adapter never frees, fail like a stuck node (same cap as judge hard stop).
pub(crate) const GRAPH_DEFER_MAX_WAIT: Duration = Duration::from_secs(270);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeferPoll {
    Empty,
    NotYet,
    TimedOut(NodeAssignment),
    Ready(NodeAssignment),
}

/// Pure deferral queue and backoff timing for graph execution nodes waiting on busy runners.
#[derive(Debug, Clone, Default)]
pub(crate) struct GraphDeferQueue {
    starts: HashMap<Agent, VecDeque<NodeAssignment>>,
    retry_after: HashMap<String, Instant>,
    since: HashMap<String, Instant>,
}

impl GraphDeferQueue {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.starts.clear();
        self.retry_after.clear();
        self.since.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.starts.values().all(|queue| queue.is_empty())
    }

    pub(crate) fn has_queued(&self, agent: Agent) -> bool {
        self.starts
            .get(&agent)
            .is_some_and(|queue| !queue.is_empty())
    }

    pub(crate) fn agents(&self) -> Vec<Agent> {
        self.starts.keys().copied().collect()
    }

    pub(crate) fn remove_node(&mut self, node_id: &str) {
        for queue in self.starts.values_mut() {
            queue.retain(|assignment| assignment.node_id != node_id);
        }
    }

    pub(crate) fn defer(&mut self, agent: Agent, assignment: NodeAssignment, now: Instant) {
        let node_id = assignment.node_id.clone();
        self.since.entry(node_id.clone()).or_insert(now);
        self.retry_after
            .insert(node_id.clone(), now + GRAPH_DEFER_RETRY_INTERVAL);
        let queue = self.starts.entry(agent).or_default();
        if !queue.iter().any(|pending| pending.node_id == node_id) {
            queue.push_back(assignment);
        }
    }

    pub(crate) fn mark_waiting_behind_busy(&mut self, agent: Agent, now: Instant) {
        if let Some(queue) = self.starts.get(&agent) {
            for pending in queue {
                self.since.insert(pending.node_id.clone(), now);
            }
        }
    }

    pub(crate) fn poll(&mut self, agent: Agent, now: Instant) -> DeferPoll {
        let Some(queue) = self.starts.get_mut(&agent) else {
            return DeferPoll::Empty;
        };
        let Some(assignment) = queue.front().cloned() else {
            return DeferPoll::Empty;
        };
        if self
            .retry_after
            .get(&assignment.node_id)
            .is_some_and(|retry_after| now < *retry_after)
        {
            return DeferPoll::NotYet;
        }
        if self
            .since
            .get(&assignment.node_id)
            .is_some_and(|since| now.saturating_duration_since(*since) >= GRAPH_DEFER_MAX_WAIT)
        {
            queue.pop_front();
            self.since.remove(&assignment.node_id);
            self.retry_after.remove(&assignment.node_id);
            return DeferPoll::TimedOut(assignment);
        }
        queue.pop_front();
        DeferPoll::Ready(assignment)
    }

    pub(crate) fn node_started(&mut self, node_id: &str) {
        self.since.remove(node_id);
        self.retry_after.remove(node_id);
    }

    #[cfg(test)]
    pub(crate) fn set_since_for_test(&mut self, node_id: impl Into<String>, since: Instant) {
        self.since.insert(node_id.into(), since);
    }

    #[cfg(test)]
    pub(crate) fn set_retry_after_for_test(
        &mut self,
        node_id: impl Into<String>,
        retry_after: Instant,
    ) {
        self.retry_after.insert(node_id.into(), retry_after);
    }

    #[cfg(test)]
    pub(crate) fn queued_for_test(&self, agent: Agent) -> Option<&VecDeque<NodeAssignment>> {
        self.starts.get(&agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dmuxai::graph::NodeRole;

    fn sample_assignment(node_id: &str, runner: &str) -> NodeAssignment {
        NodeAssignment {
            node_id: node_id.to_string(),
            runner: runner.to_string(),
            model_id: "test-model".to_string(),
            prompt: format!("prompt for {node_id}"),
            role: NodeRole::Implement,
        }
    }

    // TC-2040
    #[test]
    fn tc_2040_defer_sets_since_once_and_retry_after() {
        let mut queue = GraphDeferQueue::new();
        let t0 = Instant::now();
        let assignment = sample_assignment("step-1", "cursor");

        queue.defer(Agent::Cursor, assignment.clone(), t0);
        assert_eq!(queue.since.get("step-1"), Some(&t0));
        assert_eq!(
            queue.retry_after.get("step-1"),
            Some(&(t0 + GRAPH_DEFER_RETRY_INTERVAL))
        );
        assert_eq!(queue.starts.get(&Agent::Cursor).map(|q| q.len()), Some(1));

        let t1 = t0 + Duration::from_secs(10);
        queue.defer(Agent::Cursor, assignment, t1);
        // Original `since` is preserved
        assert_eq!(queue.since.get("step-1"), Some(&t0));
        // `retry_after` is updated
        assert_eq!(
            queue.retry_after.get("step-1"),
            Some(&(t1 + GRAPH_DEFER_RETRY_INTERVAL))
        );
        // No duplicate queue entry
        assert_eq!(queue.starts.get(&Agent::Cursor).map(|q| q.len()), Some(1));
    }

    // TC-2041
    #[test]
    fn tc_2041_poll_not_yet_ready_timed_out() {
        let mut queue = GraphDeferQueue::new();
        let t0 = Instant::now();
        let a1 = sample_assignment("step-1", "cursor");

        queue.defer(Agent::Cursor, a1.clone(), t0);

        // NotYet when now < retry_after
        let future_retry = t0 + Duration::from_secs(5);
        queue.set_retry_after_for_test("step-1", future_retry);
        assert_eq!(queue.poll(Agent::Cursor, t0), DeferPoll::NotYet);
        assert_eq!(queue.starts.get(&Agent::Cursor).map(|q| q.len()), Some(1));

        // Ready when now >= retry_after and now - since < MAX_WAIT
        let t_ready = t0 + Duration::from_secs(10);
        assert_eq!(queue.poll(Agent::Cursor, t_ready), DeferPoll::Ready(a1));
        // Ready pops from queue, but does not clear since or retry_after
        assert!(queue.starts.get(&Agent::Cursor).unwrap().is_empty());
        assert!(queue.since.contains_key("step-1"));
        assert!(queue.retry_after.contains_key("step-1"));

        // Empty when queue is drained
        assert_eq!(queue.poll(Agent::Cursor, t_ready), DeferPoll::Empty);

        // TimedOut when now - since >= MAX_WAIT
        let a2 = sample_assignment("step-2", "cursor");
        queue.defer(Agent::Cursor, a2.clone(), t0);
        queue.set_retry_after_for_test("step-2", t0);
        let t_timeout = t0 + GRAPH_DEFER_MAX_WAIT + Duration::from_secs(1);
        assert_eq!(
            queue.poll(Agent::Cursor, t_timeout),
            DeferPoll::TimedOut(a2)
        );
        // TimedOut pops from queue and cleans up since / retry_after
        assert!(queue.starts.get(&Agent::Cursor).unwrap().is_empty());
        assert!(!queue.since.contains_key("step-2"));
        assert!(!queue.retry_after.contains_key("step-2"));
    }

    // TC-2042
    #[test]
    fn tc_2042_mark_waiting_behind_busy_resets_since() {
        let mut queue = GraphDeferQueue::new();
        let t0 = Instant::now();
        let a1 = sample_assignment("step-1", "cursor");
        let a2 = sample_assignment("step-2", "cursor");
        let a3 = sample_assignment("step-3", "codex");

        queue.defer(Agent::Cursor, a1, t0);
        queue.defer(Agent::Cursor, a2, t0);
        queue.defer(Agent::Codex, a3, t0);

        assert_eq!(queue.since.get("step-1"), Some(&t0));
        assert_eq!(queue.since.get("step-2"), Some(&t0));
        assert_eq!(queue.since.get("step-3"), Some(&t0));

        let t1 = t0 + Duration::from_secs(60);
        queue.mark_waiting_behind_busy(Agent::Cursor, t1);

        // All queued nodes for Agent::Cursor have their since reset to t1
        assert_eq!(queue.since.get("step-1"), Some(&t1));
        assert_eq!(queue.since.get("step-2"), Some(&t1));
        // Other agents are unaffected
        assert_eq!(queue.since.get("step-3"), Some(&t0));
    }

    // TC-2043
    #[test]
    fn tc_2043_remove_node_and_node_started() {
        let mut queue = GraphDeferQueue::new();
        let t0 = Instant::now();
        let a1 = sample_assignment("step-1", "cursor");
        let a2 = sample_assignment("step-2", "cursor");
        let a1_codex = sample_assignment("step-1", "codex");

        queue.defer(Agent::Cursor, a1, t0);
        queue.defer(Agent::Cursor, a2, t0);
        queue.defer(Agent::Codex, a1_codex, t0);

        assert!(queue.has_queued(Agent::Cursor));
        assert!(queue.has_queued(Agent::Codex));

        // remove_node drops step-1 from all agent queues
        queue.remove_node("step-1");
        assert_eq!(queue.starts.get(&Agent::Cursor).map(|q| q.len()), Some(1));
        assert_eq!(
            queue.starts.get(&Agent::Cursor).unwrap()[0].node_id,
            "step-2"
        );
        assert_eq!(queue.starts.get(&Agent::Codex).map(|q| q.len()), Some(0));
        // remove_node does not remove since or retry_after
        assert!(queue.since.contains_key("step-1"));
        assert!(queue.retry_after.contains_key("step-1"));

        // node_started cleans up since and retry_after for the started node
        queue.node_started("step-1");
        assert!(!queue.since.contains_key("step-1"));
        assert!(!queue.retry_after.contains_key("step-1"));
        assert!(queue.since.contains_key("step-2"));
        assert!(queue.retry_after.contains_key("step-2"));
    }
}
