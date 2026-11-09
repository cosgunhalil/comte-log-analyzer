//! Masks sensitive values before a log is saved or shared (DESIGN §6.2).
//!
//! Every rule comes from something found in real Unity logs: Windows user
//! names in paths, launcher auth arguments (`-AUTH_PASSWORD=…`), the Editor's
//! `-accessToken` (whose value sits on the *next* line), licensing ids,
//! emails, LAN IPs, machine names and per-user ids in save paths.
//!
//! Redaction keeps the log readable and parseable: values are replaced with
//! `<user>`, `<redacted>`, `<token>`, `<email>`, `<ip>`, `<host>` or `<id>`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};

macro_rules! rule {
    ($name:ident = $pattern:expr) => {
        static $name: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("valid redaction regex"));
    };
}

rule!(
    INLINE_ARG = r#"(?i)(-(?:auth_password|auth_login|epicuserid|epicusername|epicsandboxid|epicdeploymentid|epicovt|accesstoken|hubsessionid|sessionid|token|password)\s*=\s*)("[^"]*"|[^\s,]+)"#
);
rule!(
    LICENSING_ID =
        r"(?i)^(\s*(?:Machine Id|Session Id|Correlation Id|External correlation Id)\s*:\s*)\S.*$"
);
rule!(ACCESS_TOKEN = r#"(?i)(access[ _]?token[:=\s]+"?)([A-Za-z0-9+/._=-]{6,})"#);
rule!(BEARER = r"(?i)(\bbearer\s+)([A-Za-z0-9._~+/=-]{16,})");
rule!(
    WINDOWS_USER_PATH =
        r#"(?i)\b([A-Z]:[\\/](?:Users|Documents and Settings)[\\/])([^\\/:*?"<>|\r\n]+?)([\\/]|$)"#
);
rule!(UNIX_USER_PATH = r"(/(?:home|Users)/)([^/\s'\x22]+)");
rule!(USER_NAME_FIELD = r"(?i)(user ?name\s*[:=]\s*')([^']*)(')");
// Identity fields games log about the player, in text (`displayName = x`) or
// JSON (`"playerName":"x"`): names and platform account ids.
rule!(
    IDENTITY_FIELD = r#"(?i)\b((?:display|player|user|nick|account|persona|profile)[ _]?name|(?:epic|steam|eos|account|user|player|profile)[ _]?id)("?\s*[:=]\s*"?)([^"',\]\s}]+)"#
);
rule!(EMAIL = r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}");
rule!(
    IPV4 = r"\b(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\b"
);
rule!(MACHINE_NAME = r"\b(?:DESKTOP|LAPTOP|WIN)-[A-Z0-9]{5,}\b");
rule!(PATH_ID = r"([\\/])([0-9a-fA-F]{32})([\\/])");

/// Command-line flags whose value is the following argument (one per line in
/// the Editor's `COMMAND LINE ARGUMENTS` block).
const VALUE_ON_NEXT_LINE: &[&str] = &[
    "-accesstoken",
    "-hubsessionid",
    "-licensingipc",
    "-token",
    "-password",
    "-auth_password",
    "-sessionid",
    "-username",
];

/// Path segments after `Users\` that are not personal.
const NOT_USER_NAMES: &[&str] = &["public", "default", "all users", "<user>", "default user"];

/// How many values were replaced, per rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactionReport {
    pub total: usize,
    pub by_rule: BTreeMap<&'static str, usize>,
}

impl RedactionReport {
    fn add(&mut self, rule: &'static str, count: usize) {
        if count > 0 {
            self.total += count;
            *self.by_rule.entry(rule).or_default() += count;
        }
    }

    /// `"3 user names, 1 token"`-style summary for the UI.
    pub fn describe(&self) -> String {
        if self.total == 0 {
            return "nothing to redact".to_owned();
        }
        self.by_rule
            .iter()
            .map(|(rule, count)| format!("{count} {rule}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Redacts lines in order (some rules look at the previous line).
#[derive(Debug, Clone)]
pub struct Redactor {
    user_names: Vec<Regex>,
    value_on_next_line: bool,
    report: RedactionReport,
}

impl Redactor {
    /// A redactor that also knows the user names appearing in `lines`' paths,
    /// so they are masked everywhere (e.g. `LicenseClient-<user>`), and any
    /// `extra_user_names` (such as the current Windows user).
    pub fn for_lines<S: AsRef<str>>(lines: &[S], extra_user_names: &[&str]) -> Self {
        // Names the user gave (or the current account) match in any case.
        let given: Vec<String> = extra_user_names
            .iter()
            .map(|n| n.trim().to_owned())
            .filter(|n| is_user_name(n))
            .collect();
        // Names learned from the log match only in the case they appeared in:
        // a Windows account named "player" must not turn "Unity Player" into
        // "Unity <user>".
        let mut learned: Vec<String> = Vec::new();
        for line in lines {
            let line = line.as_ref();
            for caps in WINDOWS_USER_PATH.captures_iter(line) {
                push_name(&mut learned, &caps[2]);
            }
            for caps in UNIX_USER_PATH.captures_iter(line) {
                push_name(&mut learned, &caps[2]);
            }
            // Player names logged as fields also appear as free text elsewhere.
            for caps in IDENTITY_FIELD.captures_iter(line) {
                if caps[1].to_ascii_lowercase().contains("name") && caps[3].len() >= 3 {
                    push_name(&mut learned, &caps[3]);
                }
            }
            for caps in INLINE_ARG.captures_iter(line) {
                if caps[1].to_ascii_lowercase().contains("username") {
                    push_name(&mut learned, caps[2].trim_matches('"'));
                }
            }
        }
        learned.retain(|n| !given.iter().any(|g| g.eq_ignore_ascii_case(n)));
        let mut patterns: Vec<(usize, String)> = given
            .iter()
            .map(|n| (n.len(), format!(r"(?i)\b{}\b", regex::escape(n))))
            .chain(
                learned
                    .iter()
                    .map(|n| (n.len(), format!(r"\b{}\b", regex::escape(n)))),
            )
            .collect();
        // Longest first, so "john smith" wins over "john".
        patterns.sort_by_key(|(len, _)| std::cmp::Reverse(*len));
        let user_names = patterns
            .iter()
            .filter_map(|(_, pattern)| Regex::new(pattern).ok())
            .collect();
        Self {
            user_names,
            value_on_next_line: false,
            report: RedactionReport::default(),
        }
    }

    pub fn report(&self) -> &RedactionReport {
        &self.report
    }

    /// Redacts one line. Lines must be passed in file order.
    pub fn redact_line<'a>(&mut self, line: &'a str) -> Cow<'a, str> {
        if std::mem::take(&mut self.value_on_next_line) && !line.trim().is_empty() {
            self.report.add("launcher arguments", 1);
            return Cow::Borrowed("<redacted>");
        }
        let flag = line.trim().to_ascii_lowercase();
        self.value_on_next_line = VALUE_ON_NEXT_LINE.contains(&flag.as_str());

        let mut text = Cow::Borrowed(line);
        let report = &mut self.report;
        let mut apply = |text: &mut Cow<'a, str>,
                         rule: &'static str,
                         re: &Regex,
                         with: &dyn Fn(&Captures) -> String| {
            let mut count = 0;
            let replaced = re.replace_all(text, |caps: &Captures| {
                count += 1;
                with(caps)
            });
            if count > 0 {
                report.add(rule, count);
                *text = Cow::Owned(replaced.into_owned());
            }
        };

        apply(&mut text, "launcher arguments", &INLINE_ARG, &|c| {
            format!("{}<redacted>", &c[1])
        });
        apply(&mut text, "licensing ids", &LICENSING_ID, &|c| {
            format!("{}<redacted>", &c[1])
        });
        apply(&mut text, "tokens", &ACCESS_TOKEN, &|c| {
            format!("{}<token>", &c[1])
        });
        apply(&mut text, "tokens", &BEARER, &|c| {
            format!("{}<token>", &c[1])
        });
        apply(&mut text, "user names", &WINDOWS_USER_PATH, &|c| {
            if is_user_name(&c[2]) {
                format!("{}<user>{}", &c[1], &c[3])
            } else {
                c[0].to_owned()
            }
        });
        apply(&mut text, "user names", &UNIX_USER_PATH, &|c| {
            format!("{}<user>", &c[1])
        });
        apply(&mut text, "user names", &USER_NAME_FIELD, &|c| {
            format!("{}<user>{}", &c[1], &c[3])
        });
        apply(&mut text, "player identities", &IDENTITY_FIELD, &|c| {
            let value = &c[3];
            if value == "0" || value.starts_with('<') {
                c[0].to_owned() // no identity in "steamId = 0" or an already-masked value
            } else if c[1].to_ascii_lowercase().contains("name") {
                format!("{}{}<user>", &c[1], &c[2])
            } else {
                format!("{}{}<id>", &c[1], &c[2])
            }
        });
        for name in &self.user_names {
            apply(&mut text, "user names", name, &|_| "<user>".to_owned());
        }
        apply(&mut text, "emails", &EMAIL, &|_| "<email>".to_owned());
        apply(&mut text, "IP addresses", &IPV4, &|c| {
            let ip = &c[0];
            if ip.starts_with("127.") || ip == "0.0.0.0" {
                ip.to_owned() // loopback says nothing about anyone
            } else {
                "<ip>".to_owned()
            }
        });
        apply(&mut text, "machine names", &MACHINE_NAME, &|_| {
            "<host>".to_owned()
        });
        apply(&mut text, "ids", &PATH_ID, &|c| {
            format!("{}<id>{}", &c[1], &c[3])
        });
        text
    }
}

fn is_user_name(name: &str) -> bool {
    let name = name.trim();
    name.chars().count() >= 2 && !NOT_USER_NAMES.contains(&name.to_lowercase().as_str())
}

fn push_name(names: &mut Vec<String>, name: &str) {
    let name = name.trim();
    if is_user_name(name) && !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        names.push(name.to_owned());
    }
}

/// Redacts all lines, returning them with a report.
pub fn redact_lines<S: AsRef<str>>(
    lines: &[S],
    extra_user_names: &[&str],
) -> (Vec<String>, RedactionReport) {
    let mut redactor = Redactor::for_lines(lines, extra_user_names);
    let out = lines
        .iter()
        .map(|l| redactor.redact_line(l.as_ref()).into_owned())
        .collect();
    let report = redactor.report().clone();
    (out, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redact(lines: &[&str]) -> Vec<String> {
        redact_lines(lines, &[]).0
    }

    #[test]
    fn windows_user_names_in_paths_and_elsewhere() {
        let out = redact(&[
            r"Successfully changed project path to: C:\Users\cosgu\OneDrive\Documents\Projects\Spirit Cabinet",
            "C:/Users/cosgu/OneDrive/Documents/Projects/Spirit Cabinet",
            r#"[Licensing::IpcConnector] Successfully connected to: "LicenseClient-cosgu" at "2026-11-05T17:59:24Z""#,
            r"Mono path[0] = 'C:/Users/John Smith/Games/X_Data/Managed'",
            r"C:\Users\Public\Documents\shared.txt",
        ]);
        assert_eq!(
            out[0],
            r"Successfully changed project path to: C:\Users\<user>\OneDrive\Documents\Projects\Spirit Cabinet"
        );
        assert_eq!(
            out[1],
            "C:/Users/<user>/OneDrive/Documents/Projects/Spirit Cabinet"
        );
        assert!(out[2].contains("\"LicenseClient-<user>\""), "{}", out[2]);
        assert_eq!(
            out[3],
            r"Mono path[0] = 'C:/Users/<user>/Games/X_Data/Managed'"
        );
        assert_eq!(
            out[4], r"C:\Users\Public\Documents\shared.txt",
            "Public is not a person"
        );
    }

    #[test]
    fn editor_command_line_values_on_the_next_line() {
        let out = redact(&[
            "COMMAND LINE ARGUMENTS:",
            "-projectpath",
            r"C:\Users\cosgu\Projects\Game",
            "-hubSessionId",
            "34e4587d-15c4-4487-8c7c-cd59b9840af3",
            "-accessToken",
            "kJw97znslGb1wF1HNKGIUmowjaOhVtI7vdvIxlOppSE011f",
            "-useHub",
        ]);
        assert_eq!(out[2], r"C:\Users\<user>\Projects\Game");
        assert_eq!(out[4], "<redacted>");
        assert_eq!(out[6], "<redacted>");
        assert_eq!(out[7], "-useHub", "the flag after a value is untouched");
    }

    #[test]
    fn epic_launcher_arguments() {
        let out = redact(&[
            r#"Epic: Full command line: C:/Program Files/Epic Games/Windbound/Windbound.exe,-AUTH_LOGIN=unused,-AUTH_PASSWORD=2966735afaa247d69faaaa5a2e064762,-AUTH_TYPE=exchangecode,-epicapp=a95bf4df6627431ca5baff8173c59635,-epicenv=Prod,-EpicPortal,-epicusername="cosgun.halil",-epicuserid=b120cfa6c170410c8ab6f6fb469fa589,-epiclocale=en"#,
            "Found command line argument -epicuserid=b120cfa6c170410c8ab6f6fb469fa589",
            r#"No platform profile, message: System.IO.DirectoryNotFoundException: Could not find a part of the path "C:\Users\cosgu\AppData\LocalLow\5 Lives Studios\Windbound\Saves\b120cfa6c170410c8ab6f6fb469fa589\Profile.json"."#,
        ]);
        assert!(out[0].contains("-AUTH_PASSWORD=<redacted>,"), "{}", out[0]);
        assert!(out[0].contains("-epicusername=<redacted>,"));
        assert!(out[0].contains("-epicuserid=<redacted>,"));
        assert!(
            out[0].contains("-AUTH_TYPE=exchangecode"),
            "non-secret args stay"
        );
        assert!(!out[0].contains("cosgun.halil"));
        assert_eq!(out[1], "Found command line argument -epicuserid=<redacted>");
        assert!(out[2].contains(r"C:\Users\<user>\AppData"));
        assert!(out[2].contains(r"\Saves\<id>\Profile.json"));
    }

    #[test]
    fn licensing_ids_and_tokens() {
        let out = redact(&[
            "  Session Id:              b039a5e72d73492588fd8a1e580e9074",
            "  Machine Id:              Wuf1NFP0gv+zjIMxrv+Of8mrnes=",
            "  Version:                 1.18.3+d7ffd15",
            "[Licensing::Module] Successfully updated the access token kJw97znslG...",
            r#"[Licensing::Module] Successfully updated access token: "kJw97zns"... (expires: 2026-11-05T16:37:00Z)"#,
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abc",
        ]);
        assert_eq!(out[0], "  Session Id:              <redacted>");
        assert_eq!(out[1], "  Machine Id:              <redacted>");
        assert_eq!(out[2], "  Version:                 1.18.3+d7ffd15");
        assert_eq!(
            out[3],
            "[Licensing::Module] Successfully updated the access token <token>"
        );
        assert!(out[4].contains(r#"access token: "<token>""#), "{}", out[4]);
        assert_eq!(out[5], "Authorization: Bearer <token>");
    }

    #[test]
    fn network_and_machine_identifiers() {
        let out = redact(&[
            r#"Player connection [14640]  * "[IP] 192.168.1.101 [Port] 55504 [Flags] 2 [Id] WindowsEditor(7,DESKTOP-MUOR995) [Debug] 1""#,
            "Using monoOptions --debugger-agent=transport=dt_socket,address=127.0.0.1:56228",
            "Contact halil@example.com for access",
            "SymInit: Symbol-SearchPath: '.', symOptions: 534, UserName: 'player'",
        ]);
        assert!(out[0].contains("[IP] <ip> [Port]"));
        assert!(out[0].contains("WindowsEditor(7,<host>)"));
        assert!(out[1].contains("127.0.0.1:56228"), "loopback stays");
        assert_eq!(out[2], "Contact <email> for access");
        assert!(out[3].ends_with("UserName: '<user>'"));
    }

    #[test]
    fn player_identity_fields_in_text_and_json() {
        let out = redact(&[
            "NETWORK PLAYER: OnPlayerDataChanged displayName = jane.doe, steamId = 0, epicId = 00020b6e8ef84254b93efe67ddb3ffbe, playerId = 0",
            r#"{"playerId":"0","playerName":"jane.doe","steamId":0,"epicId":"00020b6e8ef84254b93efe67ddb3ffbe","isHost":true}"#,
            "Welcome back, jane.doe!",
        ]);
        assert_eq!(
            out[0],
            "NETWORK PLAYER: OnPlayerDataChanged displayName = <user>, steamId = 0, epicId = <id>, playerId = 0"
        );
        assert_eq!(
            out[1],
            r#"{"playerId":"0","playerName":"<user>","steamId":0,"epicId":"<id>","isHost":true}"#
        );
        assert_eq!(
            out[2], "Welcome back, <user>!",
            "learned name masked in free text"
        );
    }

    #[test]
    fn epic_user_name_is_learned_from_launcher_arguments() {
        let out = redact(&[
            r#"Epic: Full command line: Game.exe,-epicusername="jane.doe",-epiclocale=en"#,
            "Logged in as jane.doe",
        ]);
        assert_eq!(out[1], "Logged in as <user>");
    }

    #[test]
    fn learned_names_match_only_in_their_own_case() {
        let out = redact(&[
            "Crash report: C:/Users/player/AppData/Local/Temp/Game/Crashes",
            "Unity Player started",
            "player connected",
        ]);
        assert_eq!(
            out[0],
            "Crash report: C:/Users/<user>/AppData/Local/Temp/Game/Crashes"
        );
        assert_eq!(
            out[1], "Unity Player started",
            "a capitalized word is not the account"
        );
        assert_eq!(out[2], "<user> connected");

        // Names the user typed match in any case.
        let (typed, _) = redact_lines(&["Unity Player started"], &["player"]);
        assert_eq!(typed[0], "Unity <user> started");
    }

    #[test]
    fn ordinary_unity_lines_are_left_alone() {
        let lines = [
            "    Driver:          32.0.16.1692",
            "    com.unity.ide.rider@3.0.39 (location: Library/PackageCache/com.unity.ide.rider@4504839d03f7)",
            "Asset Pipeline Refresh (id=061c453a100b521459d8cb43e1812596): Total: 3.530 seconds",
            "  at UnityEngine.GameObject.SetActive (System.Boolean value) [0x00006] in <1b6535aa091d41cf8b3ca222ed9ceef0>:0 ",
            "Direct3D 11.0 [level 11.1]",
        ];
        let (out, report) = redact_lines(&lines, &[]);
        assert_eq!(out, lines);
        assert_eq!(report.total, 0);
        assert_eq!(report.describe(), "nothing to redact");
    }

    #[test]
    fn report_counts_per_rule_and_extra_names() {
        let (out, report) = redact_lines(
            &[
                "Hello from halil",
                r"C:\Users\halil\x",
                "-accessToken",
                "secret",
            ],
            &["halil"],
        );
        assert_eq!(
            out,
            [
                "Hello from <user>",
                r"C:\Users\<user>\x",
                "-accessToken",
                "<redacted>"
            ]
        );
        assert_eq!(report.total, 3);
        assert_eq!(report.by_rule["user names"], 2);
        assert_eq!(report.describe(), "1 launcher arguments, 2 user names");
    }
}
