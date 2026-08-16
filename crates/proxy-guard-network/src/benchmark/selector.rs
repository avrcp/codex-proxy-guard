use std::collections::HashMap;

use proxy_guard_core::{BenchmarkReport, CodexRegion, NodeId, NodeSelection};

/// Lexicographic JP > SG > US selection. Country preference is never mixed into
/// the score: the highest-scoring healthy JP node always wins over any SG/US node.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeSelector;

impl NodeSelector {
    #[must_use]
    pub fn select_best(
        reports: &[BenchmarkReport],
        names: &HashMap<NodeId, String>,
    ) -> Option<NodeSelection> {
        for region in CodexRegion::PREFERENCE {
            if let Some(report) = reports
                .iter()
                .filter(|report| report.verdict.is_healthy())
                .filter(|report| report.verified_region == region)
                .max_by(|left, right| {
                    left.score
                        .cmp(&right.score)
                        .then_with(|| right.median_header_ms.cmp(&left.median_header_ms))
                        .then_with(|| right.p95_header_ms.cmp(&left.p95_header_ms))
                        .then_with(|| right.node_id.cmp(&left.node_id))
                })
            {
                let name = names
                    .get(&report.node_id)
                    .cloned()
                    .unwrap_or_else(|| report.node_id.to_string());
                return Some(NodeSelection::from_report(report, &name));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, net::Ipv4Addr};

    use proxy_guard_core::{
        BENCHMARK_SCHEMA_VERSION, BenchmarkReport, BenchmarkVerdict, CodexRegion, NodeId,
        NodeSelection,
    };

    use super::NodeSelector;

    fn report(id: NodeId, region: CodexRegion, score: u16, healthy: bool) -> BenchmarkReport {
        BenchmarkReport {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            node_id: id,
            node_fingerprint: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .into(),
            expected_region: region,
            verified_region: region,
            first_exit_ip: Ipv4Addr::new(8, 8, 8, 8).into(),
            second_exit_ip: Ipv4Addr::new(8, 8, 8, 8).into(),
            exit_ip_stable: true,
            attempts: 5,
            successes: 5,
            median_header_ms: 80,
            p95_header_ms: 120,
            jitter_ms: 10,
            score,
            verdict: if healthy {
                BenchmarkVerdict::Healthy
            } else {
                BenchmarkVerdict::Rejected {
                    reason: proxy_guard_core::BenchmarkRejection::CountryMismatch,
                }
            },
            measured_at: chrono::Utc::now(),
        }
    }

    fn names() -> HashMap<NodeId, String> {
        HashMap::new()
    }

    #[test]
    fn jp_beats_higher_scoring_sg() {
        let jp = NodeId::new();
        let sg = NodeId::new();
        let reports = vec![
            report(jp, CodexRegion::JP, 70, true),
            report(sg, CodexRegion::SG, 99, true),
        ];
        let selection = NodeSelector::select_best(&reports, &names()).expect("selection");
        assert_eq!(selection.node_id, jp);
    }

    #[test]
    fn falls_back_to_sg_then_us() {
        let sg = NodeId::new();
        let us = NodeId::new();
        let reports = vec![
            report(sg, CodexRegion::SG, 80, true),
            report(us, CodexRegion::US, 99, true),
        ];
        assert_eq!(
            NodeSelector::select_best(&reports, &names())
                .unwrap()
                .node_id,
            sg
        );

        let us = NodeId::new();
        let reports = vec![report(us, CodexRegion::US, 75, true)];
        assert_eq!(
            NodeSelector::select_best(&reports, &names())
                .unwrap()
                .node_id,
            us
        );
    }

    #[test]
    fn rejected_nodes_are_never_selected() {
        let jp = NodeId::new();
        let sg = NodeId::new();
        let reports = vec![
            report(jp, CodexRegion::JP, 90, false),
            report(sg, CodexRegion::SG, 80, true),
        ];
        assert_eq!(
            NodeSelector::select_best(&reports, &names())
                .unwrap()
                .node_id,
            sg
        );
    }

    #[test]
    fn all_rejected_returns_none() {
        let jp = NodeId::new();
        let reports = vec![report(jp, CodexRegion::JP, 90, false)];
        assert!(NodeSelector::select_best(&reports, &names()).is_none());
    }

    #[test]
    fn highest_score_within_region_wins() {
        let jp_a = NodeId::new();
        let jp_b = NodeId::new();
        let mut names = names();
        names.insert(jp_a, "JP A".into());
        names.insert(jp_b, "JP B".into());
        let reports = vec![
            report(jp_a, CodexRegion::JP, 70, true),
            report(jp_b, CodexRegion::JP, 82, true),
        ];
        let selection: NodeSelection = NodeSelector::select_best(&reports, &names).unwrap();
        assert_eq!(selection.node_id, jp_b);
        assert_eq!(selection.name, "JP B");
    }

    #[test]
    fn equal_scores_have_a_stable_latency_then_id_tie_break() {
        let first = NodeId::new();
        let second = NodeId::new();
        let mut slower = report(first, CodexRegion::JP, 80, true);
        slower.median_header_ms = 120;
        let mut faster = report(second, CodexRegion::JP, 80, true);
        faster.median_header_ms = 90;

        for reports in [
            vec![slower.clone(), faster.clone()],
            vec![faster.clone(), slower.clone()],
        ] {
            assert_eq!(
                NodeSelector::select_best(&reports, &names())
                    .expect("selection")
                    .node_id,
                second
            );
        }

        let tied_a = report(first, CodexRegion::JP, 80, true);
        let tied_b = report(second, CodexRegion::JP, 80, true);
        let expected = first.min(second);
        assert_eq!(
            NodeSelector::select_best(&[tied_a, tied_b], &names())
                .expect("selection")
                .node_id,
            expected
        );
    }
}
