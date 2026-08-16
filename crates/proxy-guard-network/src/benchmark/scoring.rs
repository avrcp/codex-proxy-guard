use chrono::{DateTime, Utc};
use proxy_guard_core::{
    BENCHMARK_SCHEMA_VERSION, BenchmarkRejection, BenchmarkReport, BenchmarkVerdict, CodexRegion,
    ExitObservation, NodeId,
};

pub const SUCCESS_RATE_GATE: f64 = 0.80;
pub const P95_GATE_MS: u64 = 5000;

const RELIABILITY_POINTS: f64 = 45.0;
const MEDIAN_POINTS: f64 = 25.0;
const P95_POINTS: f64 = 20.0;
const JITTER_POINTS: f64 = 10.0;
const MEDIAN_CEILING_MS: u64 = 2000;
const P95_CEILING_MS: u64 = 5000;
const JITTER_CEILING_MS: u64 = 1000;
const UNSTABLE_IP_PENALTY: i64 = 8;

/// Everything the deep scan observed for one node, in a form that can be scored
/// deterministically without touching the network.
#[derive(Clone, Debug)]
pub struct DeepScanInput {
    pub node_id: NodeId,
    pub node_fingerprint: String,
    pub expected_region: CodexRegion,
    pub first: ExitObservation,
    pub second: ExitObservation,
    pub attempts: u8,
    pub successes: u8,
    pub latencies_ms: Vec<u64>,
    pub sidecar_exited: bool,
    pub measured_at: DateTime<Utc>,
}

/// Apply the hard health gates, then score. Rejected nodes carry score 0 and are
/// never eligible for selection.
#[must_use]
pub fn build_report(input: DeepScanInput) -> BenchmarkReport {
    let exit_ip_stable = input.first.ip == input.second.ip;
    let verdict = gate_verdict(&input);
    let score = if verdict.is_healthy() {
        score(&input, exit_ip_stable)
    } else {
        0
    };

    BenchmarkReport {
        schema_version: BENCHMARK_SCHEMA_VERSION,
        node_id: input.node_id,
        node_fingerprint: input.node_fingerprint,
        expected_region: input.expected_region,
        verified_region: input.second.country,
        first_exit_ip: input.first.ip,
        second_exit_ip: input.second.ip,
        exit_ip_stable,
        attempts: input.attempts,
        successes: input.successes,
        median_header_ms: median(&input.latencies_ms),
        p95_header_ms: percentile_95(&input.latencies_ms),
        jitter_ms: jitter(&input.latencies_ms),
        score,
        verdict,
        measured_at: input.measured_at,
    }
}

fn gate_verdict(input: &DeepScanInput) -> BenchmarkVerdict {
    if input.sidecar_exited {
        return BenchmarkVerdict::Rejected {
            reason: BenchmarkRejection::SidecarExited,
        };
    }
    if input.first.country != input.expected_region || input.second.country != input.expected_region
    {
        return BenchmarkVerdict::Rejected {
            reason: BenchmarkRejection::CountryMismatch,
        };
    }
    if input.first.country != input.second.country {
        return BenchmarkVerdict::Rejected {
            reason: BenchmarkRejection::UnstableCountry,
        };
    }
    let success_rate = success_rate(input);
    if success_rate < SUCCESS_RATE_GATE {
        return BenchmarkVerdict::Rejected {
            reason: BenchmarkRejection::LowSuccessRate,
        };
    }
    if percentile_95(&input.latencies_ms) > P95_GATE_MS {
        return BenchmarkVerdict::Rejected {
            reason: BenchmarkRejection::HighP95,
        };
    }
    BenchmarkVerdict::Healthy
}

fn score(input: &DeepScanInput, exit_ip_stable: bool) -> u16 {
    let success_rate = success_rate(input);
    let median_ms = median(&input.latencies_ms);
    let p95_ms = percentile_95(&input.latencies_ms);
    let jitter_ms = jitter(&input.latencies_ms);

    let reliability = RELIABILITY_POINTS * success_rate;
    let median_points = MEDIAN_POINTS * ratio_inverted(median_ms, MEDIAN_CEILING_MS);
    let p95_points = P95_POINTS * ratio_inverted(p95_ms, P95_CEILING_MS);
    let jitter_points = JITTER_POINTS * ratio_inverted(jitter_ms, JITTER_CEILING_MS);

    let mut total = (reliability + median_points + p95_points + jitter_points).round() as i64;
    if !exit_ip_stable {
        total -= UNSTABLE_IP_PENALTY;
    }
    u16::try_from(total.clamp(0, 100)).unwrap_or(100)
}

fn success_rate(input: &DeepScanInput) -> f64 {
    if input.attempts == 0 {
        return 0.0;
    }
    f64::from(input.successes) / f64::from(input.attempts)
}

fn ratio_inverted(value_ms: u64, ceiling_ms: u64) -> f64 {
    let value = value_ms.min(ceiling_ms) as f64;
    let ceiling = ceiling_ms as f64;
    if ceiling <= 0.0 {
        return 0.0;
    }
    (1.0 - value / ceiling).clamp(0.0, 1.0)
}

fn median(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    if sorted.is_empty() {
        return 0;
    }
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2
    } else {
        sorted[mid]
    }
}

fn percentile_95(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * 0.95).ceil() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn jitter(values: &[u64]) -> u64 {
    if values.len() < 2 {
        return 0;
    }
    let total: u64 = values
        .windows(2)
        .map(|pair| pair[0].abs_diff(pair[1]))
        .sum();
    total / u64::try_from(values.len() - 1).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use chrono::Utc;
    use proxy_guard_core::{BenchmarkVerdict, CodexRegion, NodeId};

    use super::{DeepScanInput, P95_GATE_MS, build_report, jitter, median, percentile_95};

    fn observation(country: CodexRegion, ip: Ipv4Addr) -> proxy_guard_core::ExitObservation {
        proxy_guard_core::ExitObservation {
            ip: ip.into(),
            country,
            observed_at: Utc::now(),
        }
    }

    fn input(attempts: u8, successes: u8, latencies: &[u64]) -> DeepScanInput {
        DeepScanInput {
            node_id: NodeId::new(),
            node_fingerprint: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .into(),
            expected_region: CodexRegion::JP,
            first: observation(CodexRegion::JP, Ipv4Addr::new(8, 8, 8, 8)),
            second: observation(CodexRegion::JP, Ipv4Addr::new(8, 8, 8, 8)),
            attempts,
            successes,
            latencies_ms: latencies.to_vec(),
            sidecar_exited: false,
            measured_at: Utc::now(),
        }
    }

    #[test]
    fn healthy_node_is_scored() {
        let report = build_report(input(5, 5, &[80, 90, 100, 85, 95]));
        assert_eq!(report.verdict, BenchmarkVerdict::Healthy);
        assert!(report.score > 0);
        assert_eq!(report.median_header_ms, 90);
    }

    #[test]
    fn country_mismatch_is_rejected() {
        let mut data = input(5, 5, &[80, 90, 100, 85, 95]);
        data.second = observation(CodexRegion::US, Ipv4Addr::new(8, 8, 4, 4));
        let report = build_report(data);
        assert!(matches!(report.verdict, BenchmarkVerdict::Rejected { .. }));
        assert_eq!(report.score, 0);
    }

    #[test]
    fn low_success_rate_is_rejected() {
        let report = build_report(input(5, 3, &[80, 90, 100]));
        assert!(matches!(report.verdict, BenchmarkVerdict::Rejected { .. }));
    }

    #[test]
    fn high_p95_is_rejected() {
        let report = build_report(input(5, 5, &[100, 200, 300, 400, 6000]));
        assert!(matches!(report.verdict, BenchmarkVerdict::Rejected { .. }));
        assert!(percentile_95(&[100, 200, 300, 400, 6000]) > P95_GATE_MS);
    }

    #[test]
    fn sidecar_exit_is_rejected() {
        let mut data = input(5, 5, &[80, 90, 100]);
        data.sidecar_exited = true;
        assert!(matches!(
            build_report(data).verdict,
            BenchmarkVerdict::Rejected { .. }
        ));
    }

    #[test]
    fn unstable_ip_applies_penalty_without_rejecting() {
        let mut data = input(5, 5, &[80, 90, 100]);
        data.second = observation(CodexRegion::JP, Ipv4Addr::new(8, 8, 4, 4));
        let report = build_report(data);
        assert_eq!(report.verdict, BenchmarkVerdict::Healthy);
        assert!(!report.exit_ip_stable);
        assert!(report.score <= 100);
    }

    #[test]
    fn median_p95_and_jitter_are_deterministic() {
        assert_eq!(median(&[3, 1, 2]), 2);
        assert_eq!(median(&[1, 2, 3, 4]), 2);
        assert_eq!(percentile_95(&[100, 200, 300, 400, 500]), 500);
        assert_eq!(jitter(&[100, 200, 300]), 100);
        assert_eq!(jitter(&[42]), 0);
    }
}
