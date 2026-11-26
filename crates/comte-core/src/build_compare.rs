//! What changed between two builds (DESIGN §5.2): "your build grew by
//! 40.0 MB, and here are the assets responsible".
//!
//! Works on [`BuildReport`]s, whatever engine wrote them. Assets are matched
//! by their path; a renamed or moved asset shows as removed and added.

use std::time::Duration;

use crate::activity::{BuildReport, SizeRow, format_bytes};
use crate::model::format_duration;

/// One asset or category, before and after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeChange {
    pub name: String,
    /// `None`: not in the earlier build.
    pub before: Option<u64>,
    /// `None`: not in the later build.
    pub after: Option<u64>,
}

impl SizeChange {
    /// Bytes gained (positive) or lost (negative).
    pub fn delta(&self) -> i64 {
        self.after.unwrap_or(0) as i64 - self.before.unwrap_or(0) as i64
    }
}

/// `+40.0 MB`, `−1.2 KB`, `±0 B`.
pub fn format_delta(delta: i64) -> String {
    match delta {
        0 => "±0 B".to_owned(),
        d if d > 0 => format!("+{}", format_bytes(d as u64)),
        d => format!("−{}", format_bytes(d.unsigned_abs())),
    }
}

/// Build A ("before") against build B ("after").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildComparison {
    /// "Complete build size", A and B.
    pub total: (Option<u64>, Option<u64>),
    /// "Total User Assets", A and B.
    pub user_assets: (Option<u64>, Option<u64>),
    pub duration: (Option<Duration>, Option<Duration>),
    /// Number of assets listed, A and B.
    pub asset_count: (usize, usize),
    /// Every category in either build, largest change first.
    pub categories: Vec<SizeChange>,
    /// Assets in both builds that got bigger, largest change first.
    pub grew: Vec<SizeChange>,
    /// Assets in both builds that got smaller, largest change first.
    pub shrank: Vec<SizeChange>,
    /// Assets only in B, largest first.
    pub added: Vec<SizeChange>,
    /// Assets only in A, largest first.
    pub removed: Vec<SizeChange>,
    /// Assets in both with the same size.
    pub unchanged: usize,
}

fn by_name(rows: &[SizeRow]) -> std::collections::HashMap<&str, u64> {
    let mut map = std::collections::HashMap::new();
    for row in rows {
        // Listed twice (shouldn't happen): count both.
        *map.entry(row.name.as_str()).or_insert(0) += row.bytes;
    }
    map
}

fn largest_change_first(list: &mut [SizeChange]) {
    list.sort_by(|a, b| {
        b.delta()
            .unsigned_abs()
            .cmp(&a.delta().unsigned_abs())
            .then_with(|| a.name.cmp(&b.name))
    });
}

impl BuildComparison {
    pub fn of(a: &BuildReport, b: &BuildReport) -> Self {
        let mut c = BuildComparison {
            total: (a.total, b.total),
            user_assets: (a.user_assets, b.user_assets),
            duration: (a.duration, b.duration),
            asset_count: (a.assets.len(), b.assets.len()),
            ..Self::default()
        };

        let (cat_a, cat_b) = (by_name(&a.categories), by_name(&b.categories));
        let mut names: Vec<&str> = a.categories.iter().map(|r| r.name.as_str()).collect();
        for row in &b.categories {
            if !names.contains(&row.name.as_str()) {
                names.push(&row.name);
            }
        }
        c.categories = names
            .into_iter()
            .map(|name| SizeChange {
                name: name.to_owned(),
                before: cat_a.get(name).copied(),
                after: cat_b.get(name).copied(),
            })
            .collect();
        largest_change_first(&mut c.categories);

        let (assets_a, assets_b) = (by_name(&a.assets), by_name(&b.assets));
        for row in &b.assets {
            let name = row.name.as_str();
            let after = assets_b[name];
            let change = SizeChange {
                name: name.to_owned(),
                before: assets_a.get(name).copied(),
                after: Some(after),
            };
            match change.before {
                None if !c.added.iter().any(|x| x.name == name) => c.added.push(change),
                None => {}
                Some(before) if before < after => c.grew.push(change),
                Some(before) if before > after => c.shrank.push(change),
                Some(_) => c.unchanged += 1,
            }
        }
        for row in &a.assets {
            let name = row.name.as_str();
            if !assets_b.contains_key(name) && !c.removed.iter().any(|x| x.name == name) {
                c.removed.push(SizeChange {
                    name: name.to_owned(),
                    before: Some(assets_a[name]),
                    after: None,
                });
            }
        }
        for list in [&mut c.grew, &mut c.shrank, &mut c.added, &mut c.removed] {
            largest_change_first(list);
        }
        c
    }

    /// Both builds list sizes, so there is something to compare.
    pub fn comparable(&self) -> bool {
        (self.total.0.is_some() || self.user_assets.0.is_some() || self.asset_count.0 > 0)
            && (self.total.1.is_some() || self.user_assets.1.is_some() || self.asset_count.1 > 0)
    }

    /// How much the build grew (negative: shrank), and what that is measured
    /// on. Unity prints "Total User Assets" to 0.1 MB but the complete build
    /// size only to 0.1 GB, so your assets are the precise measure; the
    /// complete size is used when that is all there is.
    pub fn growth(&self) -> Option<(i64, &'static str)> {
        let delta = |pair: (Option<u64>, Option<u64>)| match pair {
            (Some(a), Some(b)) => Some(b as i64 - a as i64),
            _ => None,
        };
        delta(self.user_assets)
            .map(|d| (d, "your assets"))
            .or_else(|| delta(self.total).map(|d| (d, "the complete build")))
            .or_else(|| {
                (self.asset_count.0 > 0 && self.asset_count.1 > 0)
                    .then(|| (self.listed_delta(), "the listed assets"))
            })
    }

    /// The change summed over every listed asset.
    pub fn listed_delta(&self) -> i64 {
        [&self.grew, &self.shrank, &self.added, &self.removed]
            .iter()
            .flat_map(|list| list.iter())
            .map(SizeChange::delta)
            .sum()
    }

    /// "Your build grew by 40.0 MB" / "shrank by" / "is the same size".
    pub fn headline(&self) -> String {
        match self.growth() {
            None => "These builds can't be compared: one of them has no sizes".to_owned(),
            Some((0, _)) => "Your build is the same size".to_owned(),
            Some((d, _)) if d > 0 => {
                format!("Your build grew by {}", format_bytes(d as u64))
            }
            Some((d, _)) => format!("Your build shrank by {}", format_bytes(d.unsigned_abs())),
        }
    }

    /// "your assets 590.2 MB → 630.2 MB · complete build 1.20 GB → 1.24 GB ·
    /// took 9 min 14 s → 8 min 02 s"
    pub fn details(&self) -> String {
        let mut parts = Vec::new();
        let pair = |(a, b): (Option<u64>, Option<u64>)| match (a, b) {
            (Some(a), Some(b)) => Some(format!("{} → {}", format_bytes(a), format_bytes(b))),
            _ => None,
        };
        if let Some(text) = pair(self.user_assets) {
            parts.push(format!("your assets {text}"));
        }
        if let Some(text) = pair(self.total) {
            parts.push(format!("complete build {text}"));
        }
        if self.asset_count.0 > 0 || self.asset_count.1 > 0 {
            parts.push(format!(
                "{} → {} assets",
                self.asset_count.0, self.asset_count.1
            ));
        }
        if let (Some(a), Some(b)) = self.duration {
            parts.push(format!(
                "took {} → {}",
                format_duration(a),
                format_duration(b)
            ));
        }
        parts.join(" · ")
    }

    /// The asset changes that go the way the build went (growth: grown and
    /// added assets; shrinking: shrunk and removed ones), largest first.
    pub fn responsible(&self) -> Vec<&SizeChange> {
        let growing = self.growth().map_or(self.listed_delta(), |(d, _)| d) >= 0;
        let mut list: Vec<&SizeChange> = if growing {
            self.grew.iter().chain(&self.added).collect()
        } else {
            self.shrank.iter().chain(&self.removed).collect()
        };
        list.sort_by(|a, b| {
            b.delta()
                .unsigned_abs()
                .cmp(&a.delta().unsigned_abs())
                .then_with(|| a.name.cmp(&b.name))
        });
        list
    }

    /// Nothing changed in the listed assets or the totals.
    pub fn is_unchanged(&self) -> bool {
        self.grew.is_empty()
            && self.shrank.is_empty()
            && self.added.is_empty()
            && self.removed.is_empty()
            && self.growth().is_some_and(|(d, _)| d == 0)
    }

    /// A pasteable Markdown summary: the headline, the assets responsible
    /// (up to `max_rows` per list), categories, and what is new and gone.
    pub fn to_markdown(&self, before: &str, after: &str, max_rows: usize) -> String {
        let mut out = String::new();
        out.push_str(&format!("## {}\n\n", self.headline()));
        out.push_str(&format!("From **{before}** to **{after}**.\n\n"));
        let details = self.details();
        if !details.is_empty() {
            out.push_str(&format!("{details}\n\n"));
        }
        if !self.comparable() {
            return out;
        }
        let table = |out: &mut String, title: &str, rows: &[&SizeChange]| {
            if rows.is_empty() {
                return;
            }
            out.push_str(&format!(
                "### {title}\n\n| Change | Before | After | Asset |\n|---:|---:|---:|---|\n"
            ));
            for row in rows.iter().take(max_rows) {
                out.push_str(&format!(
                    "| {} | {} | {} | `{}` |\n",
                    format_delta(row.delta()),
                    row.before.map_or("—".to_owned(), format_bytes),
                    row.after.map_or("—".to_owned(), format_bytes),
                    row.name
                ));
            }
            if rows.len() > max_rows {
                out.push_str(&format!("\n…and {} more.\n", rows.len() - max_rows));
            }
            out.push('\n');
        };
        let responsible = self.responsible();
        if let Some((growth, measure)) = self.growth()
            && growth != 0
            && !responsible.is_empty()
        {
            let shown: i64 = responsible.iter().take(max_rows).map(|c| c.delta()).sum();
            out.push_str(&format!(
                "The {} largest changes account for {} of the {} change in {measure}.\n\n",
                responsible.len().min(max_rows),
                format_delta(shown),
                format_delta(growth)
            ));
        }
        table(&mut out, "Responsible", &responsible);
        let categories: Vec<&SizeChange> =
            self.categories.iter().filter(|c| c.delta() != 0).collect();
        if !categories.is_empty() {
            out.push_str(
                "### By type\n\n| Type | Before | After | Change |\n|---|---:|---:|---:|\n",
            );
            for c in categories {
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    c.name,
                    c.before.map_or("—".to_owned(), format_bytes),
                    c.after.map_or("—".to_owned(), format_bytes),
                    format_delta(c.delta())
                ));
            }
            out.push('\n');
        }
        let other: Vec<&SizeChange> = if responsible.first().is_some_and(|r| r.delta() >= 0) {
            self.shrank.iter().chain(&self.removed).collect()
        } else {
            self.grew.iter().chain(&self.added).collect()
        };
        table(&mut out, "Going the other way", &other);
        out.push_str(&format!(
            "{} new, {} gone, {} grew, {} shrank, {} unchanged.\n",
            self.added.len(),
            self.removed.len(),
            self.grew.len(),
            self.shrank.len(),
            self.unchanged
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    fn row(name: &str, mb: u64) -> SizeRow {
        SizeRow {
            name: name.to_owned(),
            bytes: mb * MB,
        }
    }

    fn build(user_mb: u64, assets: Vec<SizeRow>) -> BuildReport {
        BuildReport {
            user_assets: Some(user_mb * MB),
            total: Some(1024 * MB),
            categories: vec![row("Textures", user_mb - 10), row("Sounds", 10)],
            assets,
            ..BuildReport::default()
        }
    }

    #[test]
    fn growth_names_the_assets_responsible() {
        let a = build(
            100,
            vec![
                row("Assets/Hero.png", 20),
                row("Assets/Old.wav", 5),
                row("Assets/Same.png", 3),
            ],
        );
        let b = build(
            140,
            vec![
                row("Assets/Hero.png", 50),
                row("Assets/New.png", 12),
                row("Assets/Same.png", 3),
            ],
        );
        let c = BuildComparison::of(&a, &b);
        assert!(c.comparable());
        assert_eq!(c.headline(), "Your build grew by 40.0 MB");
        assert_eq!(c.growth(), Some((40 * MB as i64, "your assets")));
        let responsible: Vec<&str> = c.responsible().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(responsible, ["Assets/Hero.png", "Assets/New.png"]);
        assert_eq!(c.removed[0].name, "Assets/Old.wav");
        assert_eq!(c.unchanged, 1);
        assert_eq!(c.categories[0].name, "Textures");
        assert_eq!(c.categories[0].delta(), 40 * MB as i64);
        assert_eq!(format_delta(c.categories[1].delta()), "±0 B");

        let md = c.to_markdown("A", "B", 10);
        assert!(md.starts_with("## Your build grew by 40.0 MB\n"), "{md}");
        assert!(
            md.contains("| +30.0 MB | 20.0 MB | 50.0 MB | `Assets/Hero.png` |"),
            "{md}"
        );
        assert!(
            md.contains("account for +42.0 MB of the +40.0 MB change in your assets"),
            "{md}"
        );
        assert!(md.contains("### Going the other way"));
        assert!(md.contains("1 new, 1 gone, 1 grew, 0 shrank, 1 unchanged."));
    }

    #[test]
    fn shrinking_and_unchanged_builds() {
        let a = build(100, vec![row("Assets/Hero.png", 20)]);
        let b = build(90, vec![row("Assets/Hero.png", 10)]);
        let c = BuildComparison::of(&a, &b);
        assert_eq!(c.headline(), "Your build shrank by 10.0 MB");
        assert_eq!(c.responsible()[0].name, "Assets/Hero.png");

        let same = BuildComparison::of(&a, &a);
        assert!(same.is_unchanged());
        assert_eq!(same.headline(), "Your build is the same size");
    }

    #[test]
    fn a_build_without_sizes_cannot_be_compared() {
        let a = build(100, vec![row("Assets/Hero.png", 20)]);
        let b = BuildReport {
            sizes_unavailable: true,
            ..BuildReport::default()
        };
        let c = BuildComparison::of(&a, &b);
        assert!(!c.comparable());
        assert!(c.headline().contains("can't be compared"));
        assert!(!c.to_markdown("A", "B", 5).contains("###"));
    }
}
