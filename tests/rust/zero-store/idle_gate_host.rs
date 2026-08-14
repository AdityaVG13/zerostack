//! Host-measured idle-window test (ZS-OPS-006 / V6-R14).
//!
//! The unit tests prove the gate semantics with deterministic fixtures; this
//! integration test proves the *measurement*: a real `getrusage`-backed
//! sampler (in-process, no child processes, no spawn-per-call) produces
//! sealed window evidence over a real elapsed window, and the release gate
//! accepts that evidence under the V6 default budgets (<= 0.1% CPU /
//! <= 500 MB RSS). Lives outside the crate because `libc::getrusage` is
//! `unsafe` on current libc and zero-store is `#![forbid(unsafe_code)]`.

use std::time::{SystemTime, UNIX_EPOCH};

use zero_store::{
    IdleBudgetsV1, IdleGateErrorV1, IdleSampleV1, IdleSamplerV1,
    evaluate_idle_release_gate_v1, measure_idle_window_v1,
};

fn now_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

/// In-process sampler via `getrusage`. `ru_maxrss` is bytes on macOS and KB
/// on Linux. The unsafe block is sound: the struct is fully initialized
/// before the call and only this thread touches it.
struct HostIdleSamplerV1 {
    start_wall_ns: u64,
}

impl HostIdleSamplerV1 {
    fn new() -> Self {
        Self {
            start_wall_ns: now_unix_ns(),
        }
    }
}

impl IdleSamplerV1 for HostIdleSamplerV1 {
    fn sample(&mut self) -> Result<IdleSampleV1, IdleGateErrorV1> {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: `usage` is a valid, initialized `rusage` for the current
        // process; `getrusage` writes exactly one struct through the pointer.
        let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        if result != 0 {
            return Err(IdleGateErrorV1::Evaluation(
                "getrusage failed".to_owned(),
            ));
        }
        let cpu_user_ns = usage.ru_utime.tv_sec as u64 * 1_000_000_000
            + usage.ru_utime.tv_usec as u64 * 1_000;
        let cpu_sys_ns = usage.ru_stime.tv_sec as u64 * 1_000_000_000
            + usage.ru_stime.tv_usec as u64 * 1_000;
        #[cfg(target_os = "macos")]
        let rss_bytes = usage.ru_maxrss as u64;
        #[cfg(not(target_os = "macos"))]
        let rss_bytes = (usage.ru_maxrss as u64).saturating_mul(1024);
        let elapsed_wall_ns = now_unix_ns().saturating_sub(self.start_wall_ns);
        Ok(IdleSampleV1::new(
            elapsed_wall_ns,
            cpu_user_ns,
            cpu_sys_ns,
            rss_bytes,
            now_unix_ns(),
        ))
    }
}

/// A sleeping process must measure inside the V6 idle budgets: the gate
/// admits real measured evidence under default budgets, and the window
/// really elapsed.
#[test]
fn measured_idle_window_passes_default_budgets() {
    let mut sampler = HostIdleSamplerV1::new();
    let evidence = measure_idle_window_v1(&mut sampler, 80_000_000).unwrap();
    evidence.validate().unwrap();
    assert_eq!(evidence.samples.len(), 3);
    assert!(
        evidence.window_wall_ns() >= 70_000_000,
        "window must actually elapse, got {}ns",
        evidence.window_wall_ns()
    );

    let receipt = evaluate_idle_release_gate_v1(Some(&evidence), IdleBudgetsV1::default()).unwrap();
    assert!(
        receipt.admitted,
        "a sleeping process must stay within 0.1% CPU / 500MB idle budgets: {receipt:?}"
    );
    assert_eq!(receipt.samples, 3);
    assert_eq!(receipt.evidence_digest, evidence.digest().unwrap());
}

/// The measured evidence is a stable, tamper-sensitive artifact: re-sealing
/// the same window yields the same digest, and the gate refuses the same
/// evidence when the RSS budget is set below the observed peak (loud
/// refusal, never a clamp).
#[test]
fn measured_evidence_is_stable_and_budget_breach_is_loud() {
    let mut sampler = HostIdleSamplerV1::new();
    let evidence = measure_idle_window_v1(&mut sampler, 40_000_000).unwrap();
    let digest = evidence.digest().unwrap();
    assert_eq!(evidence.digest().unwrap(), digest);

    // Budget below the observed RSS: loud sealed refusal with the observed
    // value, never a silent pass.
    let observed_rss = evidence.observed_max_rss_bytes();
    let strict = IdleBudgetsV1::new(1_000_000_000, observed_rss.saturating_sub(1)).unwrap();
    let receipt = evaluate_idle_release_gate_v1(Some(&evidence), strict).unwrap();
    assert!(!receipt.admitted);
    let refusal = receipt.refusal.expect("breach must carry a refusal");
    assert_eq!(refusal.observed, observed_rss);
    assert_eq!(refusal.budget, observed_rss - 1);
}
