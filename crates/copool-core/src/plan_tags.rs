//! How a card names its plan, the one rule set the app's cards and the
//! widgets both show: its plan tag, a label and its tier color, and the
//! workspace tag beside it.
use crate::{
    models::{AccountSummary, PlanTag, PlanTags},
    pool::Pool,
};

/// When OpenAI added its $100 Pro (prolite) beside the $200 one
/// (2026-04-09 UTC).
const CODEX_PRO_LITE: i64 = 1_775_692_800;
/// When Google added its $100 Ultra beside the top one (2026-05-19 UTC).
const GOOGLE_ULTRA_LITE: i64 = 1_779_148_800;
/// When Claude renamed Max 5x to Max and Max 20x to Ultra (2026-10-30 UTC).
const CLAUDE_PLAN_RENAME: i64 = 1_793_318_400;

/// The plans whose workspace is a team's, named by the workspace tag.
const SHARED_PLANS: [&str; 3] = ["TEAM", "BUSINESS", "ENTERPRISE"];

/// A card's tags in `pool` at `now` (Unix seconds), its plans named as they
/// were then. The workspace tag is the card's source (a Sub2api provider's
/// "SUB2API"), else its team's name on a shared plan, as in the SwiftUI app.
pub(crate) fn of(pool: Pool, account: &AccountSummary, now: i64) -> PlanTags {
    let label = label(pool, reported_plan(account), now);
    let workspace = clean(account.source_tag.as_deref()).or_else(|| {
        label
            .as_deref()
            .filter(|plan| SHARED_PLANS.contains(plan))
            .and_then(|_| clean(account.team_name.as_deref()))
    });
    PlanTags {
        tag: label.map(|label| PlanTag {
            accent: accent(pool, &label),
            label,
        }),
        workspace,
    }
}

fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The plan the latest usage reports, else the login's, in capitals with
/// single spaces ("SUPERGROK HEAVY").
fn reported_plan(account: &AccountSummary) -> Option<String> {
    account
        .usage
        .as_ref()
        .and_then(|usage| clean(usage.plan_type.as_deref()))
        .or_else(|| clean(account.plan_type.as_deref()))
        .map(|plan| plan.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|plan| plan.to_uppercase())
}

/// The plan tag's label; Codex treats an unknown plan as Team, the other
/// pools show no tag until the plan is known.
fn label(pool: Pool, plan: Option<String>, now: i64) -> Option<String> {
    match pool {
        Pool::Codex => Some(codex(plan.unwrap_or_else(|| "TEAM".into()), now)),
        Pool::Claude => plan.map(|plan| claude(plan, now)),
        Pool::Antigravity => plan.map(|plan| antigravity(plan, now)),
        // Grok's plans are shown as named, Sub2api's included.
        Pool::Grok => plan,
    }
}

/// OpenAI's Pro plans, $100 (prolite), $200 (pro) and $500 (promax), are
/// named as OpenAI does once the $100 one came; Go is the $8 plan below
/// Plus. Anything else is Team.
fn codex(plan: String, now: i64) -> String {
    match plan.as_str() {
        "PROLITE" => "PRO 5X".into(),
        "PROMAX" => "PRO 50X".into(),
        "PRO" if now >= CODEX_PRO_LITE => "PRO 20X".into(),
        "GO" | "PRO" | "PLUS" | "FREE" | "TEAM" | "BUSINESS" | "ENTERPRISE" => plan,
        _ => "TEAM".into(),
    }
}

/// Claude's Max plans are named as they were at `now`: "MAX 5X" and
/// "MAX 20X" before the rename, "MAX" and "ULTRA" after. Claude Code reports
/// both as "max" with the multiplier, also since the rename; "ultra" is
/// taken should it report one. Anything else unknown is Team.
fn claude(plan: String, now: i64) -> String {
    let name = plan
        .strip_prefix("CLAUDE ")
        .or_else(|| plan.strip_prefix("CLAUDE_"))
        .unwrap_or(&plan);
    let times = match multiplier(name, "MAX") {
        Some(times) => Some(times),
        None => (name == "ULTRA").then_some(20),
    };
    match times {
        Some(times) if now >= CLAUDE_PLAN_RENAME => {
            if times >= 20 { "ULTRA" } else { "MAX" }.into()
        }
        Some(0) => "MAX".into(),
        Some(times) => format!("MAX {times}X"),
        None if ["PRO", "PLUS", "FREE", "BUSINESS", "ENTERPRISE"].contains(&name) => name.into(),
        None => "TEAM".into(),
    }
}

/// Both Ultra plans are "Google AI Ultra": known by their tier id as
/// "ULTRA 5X" and "ULTRA 20X", else as "ULTRA" alone; the top one is plain
/// Ultra before the $100 one came. Any other tier is shown as named.
fn antigravity(plan: String, now: i64) -> String {
    match plan.as_str() {
        "ULTRA 5X" | "ULTRA LITE" => "ULTRA 5X".into(),
        "ULTRA 20X" if now < GOOGLE_ULTRA_LITE => "ULTRA".into(),
        _ => plan,
    }
}

/// The multiplier of a plan named `base` or "`base` <n>X", 0 for the plain one.
fn multiplier(plan: &str, base: &str) -> Option<u32> {
    match plan.strip_prefix(base)? {
        "" => Some(0),
        rest => rest.strip_prefix(' ')?.strip_suffix('X')?.parse().ok(),
    }
}

/// A plan's color, by tier so that the pools' comparable plans match: the
/// entry subscription (Codex Go and Plus, Claude Pro, Google AI Pro,
/// SuperGrok Lite and SuperGrok) is pink; those of $100 to $300, which their
/// labels tell apart by multiplier (Codex Pro 5x and 20x, Claude Max and
/// Ultra, Google AI Ultra 5x and 20x, SuperGrok Plus and Heavy), orange;
/// Codex's $500 Pro 50x purple; Free gray; Business and Enterprise indigo;
/// Team and anything else teal.
fn accent(pool: Pool, plan: &str) -> &'static str {
    if pool == Pool::Grok {
        // Sub2api may space the name ("SUPER GROK HEAVY"). Also The X
        // Subscription's names, which may replace SuperGrok's.
        match plan.replace(' ', "").as_str() {
            "SUPERGROKLITE" | "SUPERGROK" | "PLUS" | "PREMIUM" => return "pink",
            "SUPERGROKPLUS" | "SUPERGROKHEAVY" | "SUPER" | "ULTRA" => return "orange",
            "FREE" | "BUSINESS" | "ENTERPRISE" => {}
            _ => return "teal",
        }
    }
    if pool == Pool::Codex {
        if let Some(times) = multiplier(plan, "PRO") {
            return if times >= 50 { "purple" } else { "orange" };
        }
    }
    if multiplier(plan, "MAX").is_some() || multiplier(plan, "ULTRA").is_some() {
        return "orange";
    }
    match plan {
        "GO" | "PRO" | "PLUS" => "pink",
        "FREE" => "gray",
        "BUSINESS" | "ENTERPRISE" => "indigo",
        _ => "teal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::StoredAccount;

    const BEFORE_ALL: i64 = 1_772_323_200; // 2026-03-01
    const NOW: i64 = 1_795_392_000; // 2026-11-23

    /// A card whose login names `plan` and whose usage `usage_plan`; an
    /// empty one names none.
    fn summary(plan: &str, usage_plan: &str) -> AccountSummary {
        let named = |plan: &str| (!plan.is_empty()).then(|| plan.to_owned());
        let stored: StoredAccount = serde_json::from_value(serde_json::json!({
            "id": "card", "label": "card", "accountId": "card", "authJson": {},
            "addedAt": 0, "updatedAt": 0, "planType": named(plan),
            "usage": named(usage_plan).map(|plan| serde_json::json!({"fetchedAt": 0, "planType": plan})),
        }))
        .unwrap();
        stored.to_summary(None)
    }

    #[test]
    fn every_pools_plans_are_named_and_colored_as_they_were_at_the_time() {
        use Pool::*;
        // The pool, the plan reported ("" none) and when, then the tag's
        // label ("" no tag) and color.
        let cases = [
            // Codex: an unknown or missing plan is Team.
            (Codex, "", NOW, "TEAM", "teal"),
            (Codex, " business ", NOW, "BUSINESS", "indigo"),
            (Codex, "enterprise", NOW, "ENTERPRISE", "indigo"),
            (Codex, "team", NOW, "TEAM", "teal"),
            (Codex, "free", NOW, "FREE", "gray"),
            (Codex, "go", NOW, "GO", "pink"),
            (Codex, "plus", NOW, "PLUS", "pink"),
            // OpenAI's $100 Pro came on 2026-04-09.
            (Codex, "pro", CODEX_PRO_LITE - 1, "PRO", "orange"),
            (Codex, "pro", CODEX_PRO_LITE, "PRO 20X", "orange"),
            (Codex, "prolite", NOW, "PRO 5X", "orange"),
            (Codex, "promax", NOW, "PRO 50X", "purple"),
            // Codex knows no Max or Ultra.
            (Codex, "max 20x", NOW, "TEAM", "teal"),
            (Codex, "max", NOW, "TEAM", "teal"),
            (Codex, "ultra", NOW, "TEAM", "teal"),
            (Codex, "SuperGrok", NOW, "TEAM", "teal"),
            // Claude: no tag until the plan is known.
            (Claude, "", NOW, "", ""),
            (Claude, "  ", NOW, "", ""),
            (Claude, "pro", NOW, "PRO", "pink"),
            (Claude, "free", NOW, "FREE", "gray"),
            (Claude, "enterprise", NOW, "ENTERPRISE", "indigo"),
            (Claude, "maximum", NOW, "TEAM", "teal"),
            // Claude renamed Max 5x and 20x Max and Ultra on 2026-10-30;
            // Claude Code reports both by their old names, also since.
            (
                Claude,
                "max 20x",
                CLAUDE_PLAN_RENAME - 1,
                "MAX 20X",
                "orange",
            ),
            (Claude, "max 5x", CLAUDE_PLAN_RENAME - 1, "MAX 5X", "orange"),
            (Claude, "max", CLAUDE_PLAN_RENAME - 1, "MAX", "orange"),
            (Claude, "ultra", CLAUDE_PLAN_RENAME - 1, "MAX 20X", "orange"),
            (Claude, "max 20x", CLAUDE_PLAN_RENAME, "ULTRA", "orange"),
            (Claude, "max 5x", CLAUDE_PLAN_RENAME, "MAX", "orange"),
            (Claude, "max", CLAUDE_PLAN_RENAME, "MAX", "orange"),
            (Claude, "Claude Ultra", NOW, "ULTRA", "orange"),
            (Claude, "claude_max", NOW, "MAX", "orange"),
            // Antigravity: no tag until known; Google's $100 Ultra came on
            // 2026-05-19; any other tier as named.
            (Antigravity, "", NOW, "", ""),
            (Antigravity, "pro", NOW, "PRO", "pink"),
            (
                Antigravity,
                "ultra 20x",
                GOOGLE_ULTRA_LITE - 1,
                "ULTRA",
                "orange",
            ),
            (
                Antigravity,
                "ultra 20x",
                GOOGLE_ULTRA_LITE,
                "ULTRA 20X",
                "orange",
            ),
            (Antigravity, "ultra", BEFORE_ALL, "ULTRA", "orange"),
            (Antigravity, "ultra", NOW, "ULTRA", "orange"),
            (Antigravity, "ultra 5x", NOW, "ULTRA 5X", "orange"),
            (Antigravity, "ultra lite", NOW, "ULTRA 5X", "orange"),
            (Antigravity, "standard", NOW, "STANDARD", "teal"),
            (Antigravity, "free", NOW, "FREE", "gray"),
            // Grok: as named, spaces collapsed; no tag until known.
            (Grok, "", NOW, "", ""),
            (Grok, "SuperGrok Lite", NOW, "SUPERGROK LITE", "pink"),
            (Grok, "SuperGrok", NOW, "SUPERGROK", "pink"),
            (Grok, "SuperGrok Plus", NOW, "SUPERGROK PLUS", "orange"),
            (Grok, " SuperGrok  Heavy ", NOW, "SUPERGROK HEAVY", "orange"),
            (Grok, "SUPER GROK HEAVY", NOW, "SUPER GROK HEAVY", "orange"),
            (Grok, "Premium", NOW, "PREMIUM", "pink"),
            (Grok, "Free", NOW, "FREE", "gray"),
            (Grok, "Something New", NOW, "SOMETHING NEW", "teal"),
        ];
        for (pool, plan, now, label, accent) in cases {
            let tag = of(pool, &summary(plan, ""), now).tag;
            let expected = (!label.is_empty()).then(|| PlanTag {
                label: label.into(),
                accent,
            });
            assert_eq!(tag, expected, "{pool:?} {plan:?} at {now}");
        }
    }

    #[test]
    fn the_latest_usage_names_the_plan_before_the_login() {
        let label = |pool, plan, usage_plan, now| of(pool, &summary(plan, usage_plan), now).tag;
        assert_eq!(
            label(Pool::Codex, "free", "plus", NOW).unwrap().label,
            "PLUS"
        );
        assert_eq!(label(Pool::Codex, "free", " ", NOW).unwrap().label, "FREE");
        // A Claude plan the usage reports before the login knows it.
        let claude = label(Pool::Claude, "", "max 20x", BEFORE_ALL);
        assert_eq!(claude.unwrap().label, "MAX 20X");
    }

    #[test]
    fn the_workspace_tag_is_the_source_else_a_shared_plans_team() {
        use Pool::*;
        let organization = "me@example.com's Organization";
        // The pool, the plan, the team and the source ("" none), then the
        // workspace tag ("" none).
        let cases = [
            (Codex, "business", " workspace-a ", "", "workspace-a"),
            (Codex, "enterprise", "Acme", "", "Acme"),
            (Codex, "", "Acme", "", "Acme"),
            // A personal plan's team is not shown.
            (Codex, "pro", "workspace-a", "", ""),
            (Codex, "plus", "workspace-a", "", ""),
            (Codex, "team", " ", "", ""),
            (Codex, "team", "Acme", "SUB2API", "SUB2API"),
            // Every Claude login names an organization; only a shared
            // plan's is shown.
            (Claude, "max 20x", organization, "", ""),
            (Claude, "pro", organization, "", ""),
            (Claude, "team", "Acme", "", "Acme"),
            (Claude, "maximum", "Acme", "", "Acme"),
            (Claude, "", "Acme", "", ""),
            (Claude, "", "", "SUB2API", "SUB2API"),
            (Antigravity, "pro", "", "SUB2API", "SUB2API"),
            (Antigravity, "standard", "Acme", "", ""),
            (Antigravity, "business", "Acme", "", "Acme"),
            (Grok, "SuperGrok", "", "SUB2API", "SUB2API"),
            (Grok, "SuperGrok", "Acme", " ", ""),
        ];
        let given = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        for (pool, plan, team, source, expected) in cases {
            let mut account = summary(plan, "");
            account.team_name = given(team);
            account.source_tag = given(source);
            assert_eq!(
                of(pool, &account, NOW).workspace,
                given(expected),
                "{pool:?} {plan:?} {team:?} {source:?}"
            );
        }
    }

    #[test]
    fn the_tags_serialize_as_the_app_reads_them() {
        let mut account = summary("business", "");
        account.team_name = Some("Acme".into());
        account.tags = of(Pool::Codex, &account, NOW);
        let json = serde_json::to_value(&account).unwrap();
        assert_eq!(
            json["planTag"],
            serde_json::json!({"label": "BUSINESS", "accent": "indigo"})
        );
        assert_eq!(json["workspaceTag"], "Acme");
        // A card without tags has neither key.
        account.tags = of(Pool::Claude, &summary("", ""), NOW);
        let json = serde_json::to_value(&account).unwrap();
        assert!(json.get("planTag").is_none());
        assert!(json.get("workspaceTag").is_none());
    }
}
