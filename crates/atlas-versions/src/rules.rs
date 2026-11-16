//! Mojang library rule evaluation (`rules: [{action, os}]`).

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: Action,
    pub os: Option<OsRule>,
    /// Feature-gated rules (demo mode, custom resolution…) never match
    /// during installation.
    pub features: Option<serde_json::Value>,
}

/// The platform rules are evaluated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// Mojang naming: `windows`, `osx`, `linux`.
    pub os: &'static str,
    /// `x86`, `x86_64`, `arm64`.
    pub arch: &'static str,
}

impl Platform {
    pub fn current() -> Self {
        let os = match std::env::consts::OS {
            "windows" => "windows",
            "macos" => "osx",
            _ => "linux",
        };
        let arch = match std::env::consts::ARCH {
            "x86" => "x86",
            "aarch64" => "arm64",
            _ => "x86_64",
        };
        Self { os, arch }
    }
    pub fn bits(&self) -> &'static str {
        if self.arch == "x86" { "32" } else { "64" }
    }
}

/// Launch-time feature flags (`is_demo_user`, `has_custom_resolution`, ...).
pub type Features<'a> = &'a [(&'a str, bool)];

fn features_match(r: &Rule, features: Features) -> bool {
    let Some(want) = r.features.as_ref().and_then(|f| f.as_object()) else { return true };
    want.iter().all(|(k, v)| {
        let have = features.iter().find(|(n, _)| n == k).map(|(_, b)| *b).unwrap_or(false);
        v.as_bool() == Some(have)
    })
}

fn rule_matches(r: &Rule, p: Platform) -> bool {
    rule_matches_with(r, p, &[])
}

fn rule_matches_with(r: &Rule, p: Platform, features: Features) -> bool {
    if !features_match(r, features) {
        return false;
    }
    if let Some(os) = &r.os {
        if os.name.as_deref().is_some_and(|n| n != p.os) {
            return false;
        }
        if os.arch.as_deref().is_some_and(|a| a != p.arch) {
            return false;
        }
    }
    true
}

/// Same as [`allowed`] but with launch features switched on or off.
pub fn allowed_with(rules: &[Rule], p: Platform, features: Features) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut ok = false;
    for r in rules {
        if rule_matches_with(r, p, features) {
            ok = r.action == Action::Allow;
        }
    }
    ok
}

/// Mojang semantics: no rules = allowed; otherwise start disallowed and let
/// the last matching rule decide.
pub fn allowed(rules: &[Rule], p: Platform) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut ok = false;
    for r in rules {
        if rule_matches(r, p) {
            ok = r.action == Action::Allow;
        }
    }
    ok
}
