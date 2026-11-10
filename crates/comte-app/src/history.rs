//! Timing history (DESIGN §5, "Trends"): domain reload, compile and asset
//! refresh times per Editor session, kept per project, so the overview can
//! say "domain reloads are 31% slower than your last 8 sessions".
//!
//! Stored next to the settings as `timings.toml`. A session is identified by
//! its project and its start time (both from the log), so opening the same
//! log again, or a saved session made from it, doesn't count twice.

use std::path::PathBuf;
use std::time::Duration;

use comte_core::{FactKey, LogDocument, MetricKind};
use serde::{Deserialize, Serialize};

/// Sessions kept per project.
const KEEP_PER_PROJECT: usize = 40;
/// Previous sessions a trend compares with.
const BASELINE_SESSIONS: usize = 10;
/// Slower by this much (and with enough history) is worth a warning.
const SLOWER: f64 = 0.20;

/// Count, median and slowest of one kind of operation in one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stat {
    pub count: u32,
    pub median_ms: u64,
    pub max_ms: u64,
}

impl Stat {
    fn of(durations: &[Duration]) -> Option<Self> {
        if durations.is_empty() {
            return None;
        }
        let mut ms: Vec<u64> = durations.iter().map(|d| d.as_millis() as u64).collect();
        ms.sort_unstable();
        Some(Self {
            count: u32::try_from(ms.len()).unwrap_or(u32::MAX),
            median_ms: median(&ms),
            max_ms: *ms.last().unwrap_or(&0),
        })
    }
}

fn median(sorted: &[u64]) -> u64 {
    match sorted.len() {
        0 => 0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2,
    }
}

/// One Editor session's timings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTimings {
    pub project: String,
    /// When the session started (the log's `Date:` line).
    pub started: String,
    pub unity: Option<String>,
    pub reload: Option<Stat>,
    pub compile: Option<Stat>,
    pub refresh: Option<Stat>,
}

impl SessionTimings {
    /// The timings of a log, if it says which project and when it started
    /// (Editor logs do) and timed anything.
    pub fn of(doc: &LogDocument) -> Option<Self> {
        let project = doc.fact(&FactKey::ProjectPath)?.to_owned();
        let started = doc.fact(&FactKey::StartDate)?.to_owned();
        let of_kind = |kind: MetricKind| {
            let durations: Vec<Duration> = doc
                .metrics()
                .iter()
                .filter(|m| m.kind == kind)
                .filter_map(|m| m.as_duration())
                .collect();
            Stat::of(&durations)
        };
        let timings = Self {
            project,
            started,
            unity: doc.fact(&FactKey::EngineVersion).map(str::to_owned),
            reload: of_kind(MetricKind::DomainReload),
            compile: of_kind(MetricKind::Compile),
            refresh: of_kind(MetricKind::AssetRefresh),
        };
        (timings.reload.is_some() || timings.compile.is_some() || timings.refresh.is_some())
            .then_some(timings)
    }

    fn stat(&self, kind: TrendKind) -> Option<Stat> {
        match kind {
            TrendKind::DomainReload => self.reload,
            TrendKind::Compile => self.compile,
            TrendKind::AssetRefresh => self.refresh,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingHistory {
    #[serde(default)]
    pub sessions: Vec<SessionTimings>,
}

impl TimingHistory {
    /// `timings.toml` next to `settings.toml`.
    pub fn path() -> Option<PathBuf> {
        crate::settings::Settings::path()
            .and_then(|p| p.parent().map(|dir| dir.join("timings.toml")))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string(self) {
            let _ = std::fs::write(path, text);
        }
    }

    /// Adds or updates a session. Returns whether anything changed.
    pub fn record(&mut self, timings: SessionTimings) -> bool {
        if let Some(existing) = self
            .sessions
            .iter_mut()
            .find(|s| s.project == timings.project && s.started == timings.started)
        {
            if *existing == timings {
                return false;
            }
            *existing = timings;
            return true;
        }
        let project = timings.project.clone();
        self.sessions.push(timings);
        // Oldest first per project; drop the oldest beyond the limit.
        self.sessions.sort_by(|a, b| {
            a.project
                .cmp(&b.project)
                .then_with(|| a.started.cmp(&b.started))
        });
        let mine: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.project == project)
            .map(|(i, _)| i)
            .collect();
        if mine.len() > KEEP_PER_PROJECT {
            let drop = mine.len() - KEEP_PER_PROJECT;
            let first = mine[0];
            self.sessions.drain(first..first + drop);
        }
        true
    }

    /// The trends of `current` against the project's earlier sessions.
    pub fn trends(&self, current: &SessionTimings) -> Vec<Trend> {
        let earlier: Vec<&SessionTimings> = self
            .sessions
            .iter()
            .filter(|s| s.project == current.project && s.started < current.started)
            .collect();
        TrendKind::ALL
            .into_iter()
            .filter_map(|kind| trend(kind, &earlier, current))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrendKind {
    DomainReload,
    Compile,
    AssetRefresh,
}

impl TrendKind {
    const ALL: [TrendKind; 3] = [Self::DomainReload, Self::Compile, Self::AssetRefresh];

    pub fn label(self) -> &'static str {
        match self {
            Self::DomainReload => "Domain reload",
            Self::Compile => "Script compilation",
            Self::AssetRefresh => "Asset refresh",
        }
    }
}

/// How one kind of operation compares with earlier sessions.
#[derive(Debug, Clone, PartialEq)]
pub struct Trend {
    pub kind: TrendKind,
    /// This session's median.
    pub now: Duration,
    /// The median of the earlier sessions' medians.
    pub baseline: Duration,
    /// Earlier sessions compared with.
    pub sessions: usize,
    /// now / baseline − 1 (0.31 = 31% slower).
    pub change: f64,
    /// Medians, oldest first, this session last.
    pub series: Vec<Duration>,
    /// Clearly slower, with enough history to say so.
    pub slower: bool,
}

impl Trend {
    /// "31% slower than your last 8 sessions (1.80 s)".
    pub fn sentence(&self) -> String {
        let pct = (self.change.abs() * 100.0).round();
        let sessions = if self.sessions == 1 {
            "your previous session".to_owned()
        } else {
            format!("your last {} sessions", self.sessions)
        };
        let base = comte_core::format_duration(self.baseline);
        if pct < 10.0 {
            format!("about the same as {sessions} ({base})")
        } else if self.change > 0.0 {
            format!("{pct}% slower than {sessions} ({base})")
        } else {
            format!("{pct}% faster than {sessions} ({base})")
        }
    }
}

fn trend(kind: TrendKind, earlier: &[&SessionTimings], current: &SessionTimings) -> Option<Trend> {
    let now = current.stat(kind)?;
    let previous: Vec<u64> = earlier
        .iter()
        .filter_map(|s| s.stat(kind).map(|st| st.median_ms))
        .collect();
    let previous = &previous[previous.len().saturating_sub(BASELINE_SESSIONS)..];
    if previous.is_empty() {
        return None;
    }
    let mut sorted = previous.to_vec();
    sorted.sort_unstable();
    let baseline = median(&sorted).max(1);
    let change = now.median_ms as f64 / baseline as f64 - 1.0;
    let mut series: Vec<Duration> = previous
        .iter()
        .map(|&ms| Duration::from_millis(ms))
        .collect();
    series.push(Duration::from_millis(now.median_ms));
    Some(Trend {
        kind,
        now: Duration::from_millis(now.median_ms),
        baseline: Duration::from_millis(baseline),
        sessions: previous.len(),
        change,
        series,
        slower: change >= SLOWER && previous.len() >= 3,
    })
}

/// Reloads getting slower within one session: the median of the last third
/// against the first third, when it grew by a quarter or more.
pub fn creep(durations: &[Duration]) -> Option<(Duration, Duration)> {
    if durations.len() < 4 {
        return None;
    }
    let third = durations.len() / 3;
    let med = |part: &[Duration]| {
        let mut ms: Vec<u64> = part.iter().map(|d| d.as_millis() as u64).collect();
        ms.sort_unstable();
        Duration::from_millis(median(&ms))
    };
    let first = med(&durations[..third.max(1)]);
    let last = med(&durations[durations.len() - third.max(1)..]);
    (last.as_secs_f64() >= first.as_secs_f64() * 1.25 && !first.is_zero()).then_some((first, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(started: &str, reload_ms: u64) -> SessionTimings {
        SessionTimings {
            project: "C:/Game".to_owned(),
            started: started.to_owned(),
            unity: None,
            reload: Some(Stat {
                count: 3,
                median_ms: reload_ms,
                max_ms: reload_ms * 2,
            }),
            compile: None,
            refresh: None,
        }
    }

    #[test]
    fn stats_use_the_median() {
        let ms = |v: &[u64]| {
            v.iter()
                .map(|&m| Duration::from_millis(m))
                .collect::<Vec<_>>()
        };
        let stat = Stat::of(&ms(&[1200, 900, 5000, 1000])).unwrap();
        assert_eq!((stat.count, stat.median_ms, stat.max_ms), (4, 1100, 5000));
        assert!(Stat::of(&[]).is_none());
    }

    #[test]
    fn recording_upserts_and_trends_compare_with_earlier_sessions() {
        let mut history = TimingHistory::default();
        for (day, ms) in [
            ("2026-11-01", 1000),
            ("2026-11-02", 1100),
            ("2026-11-03", 900),
        ] {
            assert!(history.record(session(day, ms)));
        }
        assert!(
            !history.record(session("2026-11-02", 1100)),
            "same session, no change"
        );
        assert!(
            history.record(session("2026-11-02", 1150)),
            "updated (live tail)"
        );
        assert_eq!(history.sessions.len(), 3);

        let now = session("2026-11-04", 1400);
        let trends = history.trends(&now);
        let reload = &trends[0];
        assert_eq!(reload.kind, TrendKind::DomainReload);
        assert_eq!(reload.baseline, Duration::from_millis(1000));
        assert_eq!(reload.sessions, 3);
        assert!(reload.slower);
        assert_eq!(
            reload.sentence(),
            "40% slower than your last 3 sessions (1.00 s)"
        );
        assert_eq!(reload.series.len(), 4);

        // Later sessions are not "earlier"; one earlier session is no warning.
        let first = history.trends(&session("2026-11-01", 1000));
        assert!(first.is_empty());
        let mut short = TimingHistory::default();
        short.record(session("2026-11-01", 1000));
        let t = &short.trends(&session("2026-11-02", 2000))[0];
        assert!(!t.slower, "one session is too little to warn");
        assert_eq!(
            t.sentence(),
            "100% slower than your previous session (1.00 s)"
        );
    }

    #[test]
    fn history_keeps_the_newest_sessions_per_project() {
        let mut history = TimingHistory::default();
        for day in 0..(KEEP_PER_PROJECT + 5) {
            history.record(session(&format!("2026-01-{day:03}"), 1000));
        }
        assert_eq!(history.sessions.len(), KEEP_PER_PROJECT);
        assert_eq!(history.sessions[0].started, "2026-01-005");
    }

    #[test]
    fn creep_within_a_session() {
        let ms = |v: &[u64]| {
            v.iter()
                .map(|&m| Duration::from_millis(m))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            creep(&ms(&[1300, 1350, 1500, 1900, 2400, 2500])),
            Some((Duration::from_millis(1325), Duration::from_millis(2450)))
        );
        assert_eq!(creep(&ms(&[1300, 1200, 1350, 1250])), None);
        assert_eq!(creep(&ms(&[1000, 3000])), None, "too few");
    }
}
