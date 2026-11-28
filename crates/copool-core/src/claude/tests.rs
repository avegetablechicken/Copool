use super::*;
use crate::application::SmartSwitchOutcome;
use crate::claude_api::*;
use crate::models::ResetKind;
use crate::pool::Response;
use crate::Preferences;
use std::sync::Mutex;

#[test]
fn fable_usage_reads_weekly_scoped_limits_and_overage_included_windows() {
    let now = 1_800_000_000;
    let reset = "2026-11-28T03:00:00Z";
    let reset_ts = reset_at(&json!(reset));

    // 1. Top-level seven_day_overage_included (official OAuth & Sub2api).
    let overage = usage_snapshot(
        &json!({
            "five_hour": {"utilization": 12.5, "resets_at": reset},
            "seven_day": {"utilization": 45.0, "resets_at": reset},
            "seven_day_overage_included": {"utilization": 56.0, "resets_at": reset}
        }),
        None,
        now,
    )
    .unwrap();
    let fable = overage.fable_one_week.as_ref().unwrap();
    assert_eq!(fable.used_percent, 56.0);
    assert_eq!(fable.window_seconds, 7 * 24 * 60 * 60);
    assert_eq!(fable.reset_at, reset_ts);

    // 2. Unused Fable quota (0%, resets_at: null) still reports the window.
    let unused = usage_snapshot(
        &json!({
            "five_hour": {"utilization": 0.0, "resets_at": null},
            "seven_day": {"utilization": 0.0, "resets_at": null},
            "seven_day_overage_included": {"utilization": 0.0, "resets_at": null}
        }),
        None,
        now,
    )
    .unwrap();
    let fable = unused.fable_one_week.as_ref().unwrap();
    assert_eq!(fable.used_percent, 0.0);
    assert_eq!(fable.reset_at, None);

    // 3. Authoritative limits[] weekly_scoped Fable wins over top-level,
    // even when is_active is false; non-Fable weekly_scoped entries are ignored.
    let scoped = usage_snapshot(
        &json!({
            "seven_day_overage_included": {"utilization": 1.0, "resets_at": null},
            "limits": [
                {
                    "kind": "weekly_scoped",
                    "group": "weekly",
                    "percent": 80,
                    "resets_at": reset,
                    "scope": {"model": {"id": "claude-sonnet-5", "display_name": "Sonnet"}},
                    "is_active": true
                },
                {
                    "kind": "weekly_scoped",
                    "group": "weekly",
                    "percent": 27.5,
                    "resets_at": reset,
                    "scope": {"model": {"id": null, "display_name": "Fable 5.2"}},
                    "is_active": false
                }
            ]
        }),
        None,
        now,
    )
    .unwrap();
    let fable = scoped.fable_one_week.as_ref().unwrap();
    assert_eq!(fable.used_percent, 27.5);
    assert_eq!(fable.reset_at, reset_ts);

    // 4. seven_day_fable fallback.
    let alias = usage_snapshot(
        &json!({"seven_day_fable": {"utilization": 33.0, "resets_at": reset}}),
        None,
        now,
    )
    .unwrap();
    assert_eq!(alias.fable_one_week.as_ref().unwrap().used_percent, 33.0);

    // 5. Accounts without Fable quota leave fable_one_week absent (and omitted in JSON).
    for body in [
        json!({"five_hour": {"utilization": 5.0}, "seven_day": {"utilization": 10.0}}),
        json!({"seven_day_overage_included": null, "seven_day_fable": null, "limits": []}),
        json!({"limits": [{"kind": "weekly_scoped", "percent": 50,
                "scope": {"model": {"display_name": "Opus"}}}]}),
    ] {
        let usage = usage_snapshot(&body, None, now).unwrap();
        assert!(usage.fable_one_week.is_none());
        let serialized = serde_json::to_value(&usage).unwrap();
        assert!(serialized.get("fableOneWeek").is_none());
    }

    // 6. Sub2api usage preserves Fable when reported and omits it otherwise.
    let sub2api_with = sub2api_usage(
        &json!({
            "five_hour": {"utilization": 0, "resets_at": null, "remaining_seconds": 0},
            "seven_day": {"utilization": 20, "resets_at": reset},
            "seven_day_overage_included": {"utilization": 15, "resets_at": reset},
            "subscription_tier": "MAX"
        }),
        now,
    )
    .unwrap();
    assert!(sub2api_with.five_hour.is_none());
    assert_eq!(
        sub2api_with.fable_one_week.as_ref().unwrap().used_percent,
        15.0
    );
    let sub2api_without = sub2api_usage(
        &json!({"seven_day": {"utilization": 20, "resets_at": reset}}),
        now,
    )
    .unwrap();
    assert!(sub2api_without.fable_one_week.is_none());
}

#[test]
fn live_reset_shape_uses_refund_flag_even_when_both_types_say_full() {
    // Read-only OAuth usage response, 2026-11-11. IDs and labels omitted.
    let body = json!({"cedar_ember": {"eligible": true, "grants": [
        {"type":"full", "resets_left":1, "ends_at":"2026-11-17T07:59:00+00:00",
         "gives_back_weekly":true, "clears":["five_hour","seven_day_overage_included"]},
        {"type":"full", "resets_left":1, "ends_at":"2026-12-10T22:00:00+00:00",
         "gives_back_weekly":false, "clears":["five_hour","seven_day","seven_day_overage_included"]}
    ]}});
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-11T00:00:00Z")
        .unwrap()
        .timestamp();
    let usage = usage_snapshot(&body, None, now).unwrap();
    assert_eq!(usage.remaining_reset_count, Some(2));
    assert_eq!(usage.reset_grants[0].kind, ResetKind::Fable);
    assert_eq!(usage.reset_grants[1].kind, ResetKind::Full);
}

#[test]
fn retained_resets_expire_without_advancing_the_confirmation_time() {
    let body = json!({"cedar_ember":{"eligible":true,"grants":[
        {"resets_left":2,"ends_at":20}, {"resets_left":1,"ends_at":30}
    ]}});
    let first = usage_snapshot(&body, None, 10).unwrap();
    let unknown = |now| usage_snapshot(&json!({}), None, now).unwrap();
    for legacy in [false, true] {
        let mut previous = first.clone();
        if legacy {
            previous.reset_grants.clear();
        }
        for (now, count) in [(20, 1), (25, 1), (30, 0), (40, 0)] {
            let mut next = unknown(now);
            crate::claude_api::retain_known_resets(&mut next, Some(&previous), &json!({}));
            assert_eq!(next.remaining_reset_count, Some(count));
            assert_eq!(next.resets_last_confirmed_at, Some(10));
            assert!(next.reset_expiries.iter().all(|at| *at > now));
            previous = next;
        }
    }
    let mut next = unknown(30);
    crate::claude_api::retain_known_resets(&mut next, Some(&unknown(10)), &json!({}));
    assert_eq!(next.remaining_reset_count, None);
    assert_eq!(next.resets_last_confirmed_at, None);
}

#[test]
fn reset_retention_distinguishes_client_capability_from_revocation() {
    let previous = usage_snapshot(
        &json!({"cedar_ember": {"eligible": true, "grants": [
            {"resets_left": 1, "ends_at": 20},
            {"resets_left": 2, "ends_at": 40},
            {"resets_left": 1}
        ]}}),
        None,
        10,
    )
    .unwrap();
    for body in [
        json!({}),
        json!({"cedar_ember": {"eligible": false, "ineligible_reason": "cli_version", "grants": []}}),
        json!({"cedar_ember": {"eligible": false, "ineligible_reason": "surface"}}),
        // Null says no more than an absent block.
        json!({"cedar_ember": null}),
    ] {
        let mut latest = usage_snapshot(&body, None, 30).unwrap();
        retain_known_resets(&mut latest, Some(&previous), &body);
        assert_eq!(latest.remaining_reset_count, Some(3), "{body}");
        assert_eq!(latest.reset_expiries, [40, 40]);
        assert_eq!(latest.resets_last_confirmed_at, Some(10));
        let mut absent = usage_snapshot(&body, None, 30).unwrap();
        retain_known_resets(&mut absent, None, &body);
        assert_eq!(absent.remaining_reset_count, None);
    }
    for block in [
        json!({"eligible": false}),
        json!({"eligible": false, "ineligible_reason": "plan"}),
        json!({"eligible": false, "ineligible_reason": "unknown"}),
        json!({"eligible": true}),
        json!({"eligible": true, "grants": "invalid"}),
        json!([]),
        json!("invalid"),
    ] {
        let body = json!({"cedar_ember": block});
        let mut latest = usage_snapshot(&body, None, 30).unwrap();
        retain_known_resets(&mut latest, Some(&previous), &body);
        assert_eq!(latest.remaining_reset_count, None, "{body}");
        assert!(latest.reset_grants.is_empty());
        assert!(latest.reset_expiries.is_empty());
        assert_eq!(latest.resets_last_confirmed_at, None);
    }
    let body = json!({"cedar_ember": {"eligible": true, "grants": []}});
    let mut latest = usage_snapshot(&body, None, 30).unwrap();
    retain_known_resets(&mut latest, Some(&previous), &body);
    assert_eq!(latest.remaining_reset_count, Some(0));
}

#[tokio::test]
async fn explicit_reset_revocation_clears_the_saved_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    let inventory = json!({"cedar_ember": {"eligible": true, "grants": [
        {"resets_left": 2, "ends_at": "2099-01-01T00:00:00Z"}
    ]}});
    fake.usage.lock().unwrap().push((200, inventory.clone()));
    app.claude_import_current().await.unwrap();
    let stored = || app.claude_load().unwrap().accounts[0].clone();
    let id = stored().id;
    for block in [
        json!({"eligible": false, "ineligible_reason": "plan"}),
        json!({"eligible": false}),
    ] {
        fake.usage.lock().unwrap().push((200, inventory.clone()));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(stored().usage.unwrap().remaining_reset_count, Some(2));
        fake.usage
            .lock()
            .unwrap()
            .push((200, json!({"cedar_ember": block})));
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        let usage = stored().usage.unwrap();
        assert_eq!(usage.remaining_reset_count, None);
        assert!(usage.reset_grants.is_empty());
        assert_eq!(usage.resets_last_confirmed_at, None);
    }
}

#[test]
fn reset_inventory_distinguishes_weekly_refunds_from_full_resets() {
    let grant = |left, ends, clears: Value, refund| {
        json!({
            "resets_left": left, "ends_at": ends, "clears": clears,
            "gives_back_weekly": refund
        })
    };
    let body = json!({"cedar_ember": {"eligible": true, "grants": [
        grant(1, 300, json!(["five_hour", "seven_day"]), false),
        grant(2, 400, json!(["five_hour", "seven_day_overage_included"]), true),
        grant(1, 500, json!(["five_hour"]), false),
        grant(1, 600, Value::Null, false),
        grant(3, 100, json!(["five_hour"]), true),
        grant(0, 700, json!(["five_hour"]), true)
    ]}});
    let usage = usage_snapshot(&body, None, 100).unwrap();
    assert_eq!(usage.remaining_reset_count, Some(5));
    assert_eq!(usage.reset_expiries, [300, 400, 400, 500, 600]);
    assert_eq!(
        usage
            .reset_grants
            .iter()
            .map(|g| g.kind)
            .collect::<Vec<_>>(),
        [
            ResetKind::Full,
            ResetKind::Fable,
            ResetKind::Partial,
            ResetKind::Unknown
        ]
    );
    assert_eq!(usage.reset_grants[1].count, 2);
    let serialized = serde_json::to_value(&usage).unwrap();
    assert_eq!(serialized["resetGrants"][1]["kind"], "fable");
    assert_eq!(serialized["resetGrants"][1]["expiresAt"], 400);
    let mut old = serialized;
    old.as_object_mut().unwrap().remove("resetGrants");
    assert!(serde_json::from_value::<UsageSnapshot>(old)
        .unwrap()
        .reset_grants
        .is_empty());
    // A future grant field must never override the weekly-refund flag.
    let refund = json!({"eligible":true,"grants":[
            grant(1, 300, json!(["five_hour", "seven_day"]), true)]});
    assert_eq!(
        banked_resets(&refund, 100).unwrap().2[0].kind,
        ResetKind::Fable
    );
    // No count beyond reason is listed reset by reset.
    let mut huge = grant(1, 300, json!(["five_hour", "seven_day"]), false);
    huge["resets_left"] = 1e18.into();
    let (count, expiries, _) = banked_resets(
        &json!({"eligible": true, "grants": [huge.clone(), huge]}),
        100,
    )
    .unwrap();
    assert_eq!((count, expiries.len()), (1000, 1000));
}

#[test]
fn banked_resets_count_what_unexpired_grants_have_left() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-12T14:30:00Z")
        .unwrap()
        .timestamp();
    // A grant as /api/oauth/usage?cedar_ember=1 returned it to a Max account.
    let grant = |left: i64, ends: &str| {
        json!({"id":"grant-id","label":"Claude Opus 5.5 launch: one usage-limit reset for Pro and Max",
                "resets_total":1,"resets_left":left,"starts_at":"2026-09-22T16:00:00+00:00","ends_at":ends,
                "clears":["five_hour","seven_day","seven_day_overage_included"],"paused":false,"usable_now":left > 0})
    };
    let usage = |block: Value| {
        usage_snapshot(
                &json!({"five_hour":{"utilization":5.0,"resets_at":"2026-10-12T17:39:59.634467+00:00"},
                    "seven_day":{"utilization":24.0,"resets_at":"2026-10-17T06:00:00.634492+00:00"},
                    "cedar_ember":block,"extra_usage":{"is_enabled":false,"monthly_limit":4000,"used_credits":1285.0,
                        "utilization":32.125,"currency":"USD","decimal_places":2,"disabled_reason":"out_of_credits"}}),
                None,
                now,
            )
            .unwrap()
    };
    let eligible = |grants: Vec<Value>| json!({"eligible":true,"ineligible_reason":null,"grants":grants,"next_grant_id":"next"});
    let live = usage(eligible(vec![grant(1, "2026-10-22T16:00:00+00:00")]));
    assert_eq!(live.remaining_reset_count, Some(1));
    assert_eq!(live.five_hour.as_ref().unwrap().used_percent, 5.0);
    assert_eq!(live.one_week.as_ref().unwrap().used_percent, 24.0);
    // Spent, expired and absent grants are a real zero.
    for grants in [
        vec![grant(0, "2026-10-22T16:00:00+00:00")],
        vec![grant(1, "2026-10-01T00:00:00+00:00")],
        vec![],
    ] {
        assert_eq!(usage(eligible(grants)).remaining_reset_count, Some(0));
    }
    let several = vec![
        grant(1, "2026-10-22T16:00:00+00:00"),
        grant(2, "2026-11-01T00:00:00+00:00"),
        grant(-3, "2026-11-01T00:00:00+00:00"),
    ];
    let several = usage(eligible(several));
    assert_eq!(several.remaining_reset_count, Some(3));
    let ends = |at: &str| {
        chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .timestamp()
    };
    assert_eq!(
        several.reset_expiries,
        [
            ends("2026-10-22T16:00:00Z"),
            ends("2026-11-01T00:00:00Z"),
            ends("2026-11-01T00:00:00Z")
        ]
    );
    assert_eq!(live.reset_expiries, [ends("2026-10-22T16:00:00Z")]);
    // An ineligible caller, or a response without the block, is unknown.
    for block in [
        json!({"eligible":false,"ineligible_reason":"surface"}),
        json!({"eligible":false,"ineligible_reason":"cli_version","grants":[]}),
        json!({"eligible":true}),
        Value::Null,
    ] {
        assert_eq!(usage(block).remaining_reset_count, None);
    }
}

#[test]
fn credit_expiries_include_paid_and_promotional_tranches_without_spending_dates() {
    let a = "2026-12-01T00:00:00Z";
    let b = "2027-01-01T00:00:00Z";
    let prepaid = json!({"amount":500, "next_expires_at":a,
            "tranches":[{"remaining_amount_minor_units":200,"expires_at":b},
                        {"remaining_amount_minor_units":0,"expires_at":"2026-11-01T00:00:00Z"}],
            "promo_tranches":[{"remaining_amount_minor_units":300,"expires_at":a},
                              {"remaining_amount_minor_units":10,"expires_at":"invalid"}]});
    let extra = json!({"monthly_limit":10000,"used_credits":0});
    let credits = extra_usage_credits(&extra, &prepaid).unwrap();
    assert_eq!(
        credits.expiries,
        [reset_at(&json!(a)).unwrap(), reset_at(&json!(b)).unwrap()]
    );
    assert_eq!(credits.balance.as_deref(), Some("$5.00"));
    assert!(prepaid_credit_expiries(&json!({"expiry_policy_months":12})).is_empty());
    assert_eq!(
        prepaid_credit_expiries(&json!({"next_expires_at":b})),
        [reset_at(&json!(b)).unwrap()]
    );
    let old: CreditSnapshot =
        serde_json::from_value(json!({"hasCredits":true,"unlimited":false,"balance":"5"})).unwrap();
    assert!(old.expiries.is_empty());
}

#[test]
fn extra_usage_credits_are_the_smaller_of_the_limit_remainder_and_the_balance() {
    // As /api/oauth/usage and /prepaid/credits returned them for a Max account.
    let extra = json!({"is_enabled":false,"monthly_limit":4000,"used_credits":1285.0,
            "currency":"USD","decimal_places":2,"disabled_reason":"out_of_credits"});
    let credits = |extra: &Value, prepaid: Value| {
        extra_usage_credits(extra, &prepaid).map(|c| (c.balance, c.unlimited, c.has_credits))
    };
    let held = |minor: i64| {
        json!({"amount":-1,"currency":"USD","balance":{"money":null,
                "credits":{"amount_minor":minor,"exponent":2}},"auto_reload_settings":null})
    };
    assert_eq!(
        credits(&extra, held(0)),
        Some((Some("$0.00".into()), false, false))
    );
    assert_eq!(
        credits(&extra, held(9000)),
        Some((Some("$27.15".into()), false, true))
    );
    assert_eq!(
        credits(&extra, held(1000)),
        Some((Some("$10.00".into()), false, true))
    );
    let money = json!({"amount":9000,"currency":"USD","balance":{"money":
            {"amount_minor":9000,"currency":"USD","exponent":2},"credits":null}});
    let uncapped = json!({"is_enabled":true,"used_credits":1285,"currency":"EUR"});
    assert_eq!(
        credits(&uncapped, money),
        Some((Some("€90.00".into()), false, true))
    );
    // Auto-reload leaves only the limit, and with neither it is unlimited.
    let reloaded = json!({"amount":200,"auto_reload_settings":{"enabled":true}});
    assert_eq!(
        credits(&extra, reloaded.clone()),
        Some((Some("$27.15".into()), false, true))
    );
    assert_eq!(credits(&uncapped, reloaded), Some((None, true, true)));
    // A bare amount is minor units; without a balance nothing is known.
    assert_eq!(
        credits(&extra, json!({"amount":500})),
        Some((Some("$5.00".into()), false, true))
    );
    assert_eq!(credits(&extra, json!({"amount":-1})), None);
    assert_eq!(credits(&extra, Value::Null), None);
    assert_eq!(credits(&Value::Null, held(9000)), None);
}

#[test]
fn server_clock_offsets_respect_time_zones() {
    let now = reset_at(&json!("2026-10-30T12:00:00Z")).unwrap();
    for date in [
        "Fri, 30 Oct 2026 18:00:00 GMT",
        "Sat, 31 Oct 2026 02:00:00 +0800",
    ] {
        assert_eq!(clock_offset_seconds(date, now), Some(6 * 3600));
    }
    assert_eq!(
        clock_offset_seconds("Fri, 30 Oct 2026 06:00:00 GMT", now),
        Some(-6 * 3600)
    );
    assert_eq!(clock_offset_seconds("invalid", now), None);
    assert_eq!(reset_at(&json!("2026-10-30T20:00:00+08:00")), Some(now));
}

#[tokio::test]
async fn usage_clock_offset_survives_storage_without_changing_reset_dates() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.accounts(Pool::Claude).await.unwrap()[0].id.clone();
    for offset in [Some(6 * 3600), Some(-6 * 3600), None] {
        fake.usage.lock().unwrap().push((
                200,
                json!({
                    "five_hour": {"utilization": 5.0, "resets_at": "2099-01-01T00:00:00Z"},
                    "seven_day": {"utilization": 15.0, "resets_at": "2099-01-01T00:00:00Z"},
                    "seven_day_overage_included": {"utilization": 25.0, "resets_at": "2099-01-01T00:00:00Z"}
                }),
            ));
        *fake.usage_clock_offset.lock().unwrap() = offset;
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        let stored = app.claude_load().unwrap();
        let usage = stored.accounts[0].usage.as_ref().unwrap();
        assert_eq!(
            usage.five_hour.as_ref().unwrap().clock_offset_seconds,
            offset
        );
        assert_eq!(
            usage.one_week.as_ref().unwrap().clock_offset_seconds,
            offset
        );
        assert_eq!(
            usage.fable_one_week.as_ref().unwrap().clock_offset_seconds,
            offset
        );
        assert_eq!(
            usage.five_hour.as_ref().unwrap().reset_at,
            reset_at(&json!("2099-01-01T00:00:00Z"))
        );
    }
}

#[tokio::test]
async fn extra_usage_credits_ask_for_the_balance_through_the_account_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.accounts(Pool::Claude).await.unwrap()[0].id.clone();
    let usage = json!({"five_hour":{"utilization":5},"extra_usage":{"is_enabled":true,
            "monthly_limit":4000,"used_credits":1285,"currency":"USD","decimal_places":2}});
    *fake.prepaid.lock().unwrap() = Some(json!({"amount":-1,"balance":{"money":null,
            "credits":{"amount_minor":1500,"exponent":2}}}));
    fake.usage.lock().unwrap().push((200, usage.clone()));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    let credits = |app: &Application| {
        app.claude_load().unwrap().accounts[0]
            .usage
            .as_ref()
            .unwrap()
            .credits
            .as_ref()
            .and_then(|c| c.balance.clone())
    };
    assert_eq!(credits(&app), Some("$15.00".into()));
    let requests = fake.prepaid_requests.lock().unwrap().clone();
    let proxy = fake.usage_proxies.lock().unwrap().last().cloned().unwrap();
    assert_eq!(requests, vec![("org-alice".to_string(), proxy)]);
    // A failed balance is unknown, and without extra usage it is not asked for.
    *fake.prepaid.lock().unwrap() = None;
    fake.usage.lock().unwrap().push((200, usage));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(credits(&app), None);
    fake.usage
        .lock()
        .unwrap()
        .push((200, json!({"five_hour":{"utilization":5}})));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(fake.prepaid_requests.lock().unwrap().len(), 2);
}

const REFRESH_SECONDS: i64 = 300;
fn queue_usage_rate_limit(fake: &Fake, body: Value) {
    fake.usage
        .lock()
        .unwrap()
        .extend([(429, Value::Null), (429, body)]);
}
/// Moves every account's last request this many seconds back.
fn age(app: &Application, seconds: i64) {
    let mut store = app.claude_load().unwrap();
    for account in &mut store.accounts {
        account.updated_at = now_unix_seconds() - seconds;
    }
    app.claude_save(&store).unwrap();
}
/// Ends a rate limit pause without forgetting how many came before.
fn end_pause(app: &Application) {
    let mut store = app.claude_load().unwrap();
    store.accounts[0].own.usage_retry_at = Some(now_unix_seconds() - 1);
    app.claude_save(&store).unwrap();
}

#[test]
fn rate_limit_pauses_double_up_to_an_hour_and_honor_a_longer_retry_after() {
    let pauses: Vec<i64> = (1..=7).map(|n| rate_limit_pause(n, None)).collect();
    assert_eq!(pauses, [300, 600, 1200, 2400, 3600, 3600, 3600]);
    assert_eq!(rate_limit_pause(u32::MAX, None), 3600);
    assert_eq!(rate_limit_pause(0, None), 300);
    // Retry-After: 0 or shorter than the backoff does not shorten it.
    assert_eq!(rate_limit_pause(1, Some(0)), 300);
    assert_eq!(rate_limit_pause(3, Some(60)), 1200);
    assert_eq!(rate_limit_pause(1, Some(5400)), 5400);
    assert_eq!(rate_limit_pause(1, Some(u64::MAX)), 6 * 3600);
}

#[tokio::test]
async fn consecutive_rate_limits_pause_longer_until_usage_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    let stored = || app.claude_load().unwrap().accounts[0].clone();
    let usage = stored().usage.unwrap();
    for (count, minutes) in [(1, 5), (2, 10), (3, 20), (4, 40), (5, 60), (6, 60)] {
        queue_usage_rate_limit(&fake, json!({"error":{"message":"Rate limited"}}));
        let before = now_unix_seconds();
        let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
        assert_eq!(
                error,
                format!("Claude usage request failed: HTTP 429 Rate limited; retry paused for {minutes} minutes")
            );
        let account = stored();
        assert_eq!(account.own.rate_limits, count);
        let until = account.own.usage_retry_at.unwrap();
        assert!((before + minutes * 60..=now_unix_seconds() + minutes * 60).contains(&until));
        // The last usage stays on the card with the error beside it.
        assert_eq!(
            serde_json::to_value(&account.usage).unwrap(),
            serde_json::to_value(Some(&usage)).unwrap()
        );
        assert_eq!(account.usage_error.as_deref(), Some(error.as_str()));
        // While paused, nothing is requested.
        let requests = usages(&fake).len();
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
            error
        );
        assert_eq!(usages(&fake).len(), requests);
        end_pause(&app);
    }
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    let account = stored();
    assert_eq!(account.own.rate_limits, 0);
    assert_eq!(account.own.usage_retry_at, None);
    assert_eq!(account.usage_error, None);
    // The count is gone from the store once it is back to zero.
    let saved = fs::read_to_string(app.claude_store_path()).unwrap();
    assert!(!saved.contains("rateLimits"));
    // A first rate limit after a success pauses briefly again.
    queue_usage_rate_limit(&fake, Value::Null);
    app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
    assert_eq!(stored().own.rate_limits, 1);
}

#[tokio::test]
async fn other_failures_keep_the_rate_limit_count_without_pausing() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    for _ in 0..2 {
        queue_usage_rate_limit(&fake, Value::Null);
        app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
        end_pause(&app);
    }
    fake.usage.lock().unwrap().push((500, Value::Null));
    assert_eq!(
        app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
        "Claude usage request failed: HTTP 500"
    );
    let account = app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(account.own.rate_limits, 2);
    assert_eq!(account.own.usage_retry_at, None);
    queue_usage_rate_limit(&fake, Value::Null);
    assert!(app
        .refresh_usage(Pool::Claude, &id)
        .await
        .unwrap_err()
        .ends_with("retry paused for 20 minutes"));
}

#[tokio::test]
async fn a_longer_retry_after_sets_the_pause() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    for (retry_after, minutes) in [(Some(5400), 90), (Some(0), 10), (None, 20)] {
        *fake.usage_retry_after.lock().unwrap() = retry_after;
        queue_usage_rate_limit(&fake, Value::Null);
        let before = now_unix_seconds();
        let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
        assert!(
            error.ends_with(&format!("retry paused for {minutes} minutes")),
            "{error}"
        );
        let until = app.claude_load().unwrap().accounts[0]
            .own
            .usage_retry_at
            .unwrap();
        assert!((before + minutes * 60..=now_unix_seconds() + minutes * 60).contains(&until));
        end_pause(&app);
    }
    // A Retry-After on a successful response sets no pause.
    *fake.usage_retry_after.lock().unwrap() = Some(9999);
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    let account = app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(account.own.usage_retry_at, None);
    assert_eq!(account.own.rate_limits, 0);
}

#[tokio::test]
async fn a_card_whose_usage_changes_speeds_up_alone_between_rounds() {
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    assert_eq!(usages(&fake).len(), 1);
    let (five, one) = (Duration::from_secs(300), Duration::from_secs(60));
    let usage = |hour: u32| (200, json!({"five_hour": {"utilization": hour}}));
    // Codex runs independently and never requests Claude.
    assert_eq!(app.background_round(0, Pool::Codex).await.as_secs(), 10);
    let codex = app.schedule(Pool::Codex).unwrap();
    // Asked at import: the first round waits, and nothing changed.
    assert_eq!(app.background_round(0, Pool::Claude).await, five);
    assert_eq!(usages(&fake).len(), 1);
    assert_eq!(app.fast_wait(Pool::Claude), None);
    // A change leaves the rounds at five minutes and speeds the card up.
    fake.usage.lock().unwrap().push(usage(30));
    age(&app, 300);
    assert_eq!(app.background_round(1, Pool::Claude).await, five);
    assert_eq!(app.round_interval(Pool::Claude), five);
    assert_eq!(usages(&fake).len(), 2);
    assert_eq!(app.fast_wait(Pool::Claude), Some(one));
    let next = app.accounts(Pool::Claude).await.unwrap()[0]
        .next_refresh_at
        .unwrap();
    let now = now_unix_seconds();
    assert!((now + 59..=now + 60).contains(&next), "{next} {now}");
    // Asked a minute later, unchanged: one unchanged refresh...
    fake.usage.lock().unwrap().push(usage(30));
    age(&app, 60);
    app.fast_round(Pool::Claude).await;
    assert_eq!(usages(&fake).len(), 3);
    assert_eq!(app.fast_wait(Pool::Claude), Some(one));
    // ...a change starts the count over...
    fake.usage.lock().unwrap().push(usage(35));
    age(&app, 60);
    app.fast_round(Pool::Claude).await;
    assert_eq!(usages(&fake).len(), 4);
    // ...an account asked under a minute ago waits, which counts...
    age(&app, 45);
    app.fast_round(Pool::Claude).await;
    assert_eq!(usages(&fake).len(), 4);
    assert_eq!(app.fast_wait(Pool::Claude), Some(one));
    // ...and the second unchanged refresh ends the speed-up.
    fake.usage.lock().unwrap().push(usage(35));
    age(&app, 60);
    app.fast_round(Pool::Claude).await;
    assert_eq!(usages(&fake).len(), 5);
    assert_eq!(app.fast_wait(Pool::Claude), None);
    assert!(app.speedup(Pool::Claude, &id).is_none());
    // A rate limit is no change in usage.
    queue_usage_rate_limit(&fake, Value::Null);
    age(&app, 300);
    assert_eq!(app.background_round(2, Pool::Claude).await, five);
    assert_eq!(app.fast_wait(Pool::Claude), None);
    assert_eq!(app.schedule(Pool::Codex), Some(codex));
    assert_eq!(app.background_round(1, Pool::Codex).await.as_secs(), 10);
    assert_eq!(usages(&fake).len(), 7);
}

#[tokio::test]
async fn a_card_without_its_own_speed_up_keeps_to_the_rounds() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    // Alice, signed in last, is in use.
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    let id = |current: bool| {
        accounts
            .iter()
            .find(|a| a.is_current == current)
            .map(|a| a.id.clone())
            .unwrap()
    };
    let (alice, bob) = (id(true), id(false));
    let mut preferences = app.preferences().unwrap();
    preferences.refresh.cards.claude.insert(
        bob.clone(),
        crate::refresh_policy::CardRefresh {
            adaptive: Some(false),
            backoff: None,
        },
    );
    app.save_preferences(preferences).await.unwrap();
    assert!(
        app.preferences()
            .unwrap()
            .refresh
            .card(Pool::Claude, &alice)
            .adaptive
    );
    assert!(!app.card_refresh(Pool::Claude, &bob).adaptive);
    // Both change in a full round; only Alice speeds up.
    let usage = (200, json!({"five_hour": {"utilization": 30}}));
    fake.usage.lock().unwrap().extend([usage.clone(), usage]);
    age(&app, 300);
    app.background_round(1, Pool::Claude).await;
    assert_eq!(usages(&fake).len(), 4);
    // As the speed-up loop, woken, schedules its next ask.
    assert!(app.fast_wait(Pool::Claude).is_some());
    assert!(app.speedup(Pool::Claude, &alice).is_some());
    assert!(app.speedup(Pool::Claude, &bob).is_none());
    age(&app, 60);
    app.fast_round(Pool::Claude).await;
    assert_eq!(usages(&fake)[4..], ["alice-access"]);
    // Turning Alice's off ends hers at the next ask, without a request.
    let mut preferences = app.preferences().unwrap();
    preferences.refresh.cards.claude.insert(
        alice.clone(),
        crate::refresh_policy::CardRefresh {
            adaptive: Some(false),
            backoff: None,
        },
    );
    app.save_preferences(preferences).await.unwrap();
    age(&app, 60);
    app.fast_round(Pool::Claude).await;
    assert_eq!(usages(&fake).len(), 5);
    assert_eq!(app.fast_wait(Pool::Claude), None);
}

#[tokio::test]
async fn background_refreshes_skip_accounts_asked_within_the_interval() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    assert_eq!(usages(&fake), ["bob-access", "alice-access"]);
    // Every round skips accounts asked within the interval.
    for tick in 0..12 {
        app.refresh_recurring(Pool::Claude, tick).await.unwrap();
    }
    assert_eq!(usages(&fake).len(), 2);
    age(&app, REFRESH_SECONDS - 5);
    app.refresh_recurring(Pool::Claude, 0).await.unwrap();
    assert_eq!(usages(&fake).len(), 2);
    // Once due, every round asks both current and inactive accounts.
    age(&app, REFRESH_SECONDS);
    app.refresh_recurring(Pool::Claude, 1).await.unwrap();
    let mut refreshed = usages(&fake)[2..].to_vec();
    refreshed.sort();
    assert_eq!(refreshed, ["alice-access", "bob-access"]);
    app.refresh_recurring(Pool::Claude, 2).await.unwrap();
    assert_eq!(usages(&fake).len(), 4);
    app.refresh_recurring(Pool::Claude, 6).await.unwrap();
    assert_eq!(usages(&fake).len(), 4);
    // A manual refresh is not held back by the interval.
    app.refresh_all(Pool::Claude).await.unwrap();
    let mut manual = usages(&fake)[4..].to_vec();
    manual.sort();
    assert_eq!(manual, ["alice-access", "bob-access"]);
    app.refresh_usage(Pool::Claude, &app.claude_load().unwrap().accounts[0].id)
        .await
        .unwrap();
    assert_eq!(usages(&fake).len(), 7);
}

#[tokio::test]
async fn rounds_between_full_ones_ask_only_for_the_account_in_use() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let mut preferences = app.preferences().unwrap();
    preferences.refresh.claude = crate::PoolRefresh {
        seconds: 120,
        full_every: 3,
        adaptive: false,
        backoff: true,
    };
    app.save_preferences(preferences).await.unwrap();
    assert_eq!(app.round_interval(Pool::Claude).as_secs(), 120);
    // Alice is in use: rounds 1 and 2 ask for her alone...
    for tick in [1, 2] {
        age(&app, 120);
        app.refresh_recurring(Pool::Claude, tick).await.unwrap();
    }
    assert_eq!(usages(&fake)[2..], ["alice-access", "alice-access"]);
    // ...and round 3 for everyone.
    age(&app, 120);
    app.refresh_recurring(Pool::Claude, 3).await.unwrap();
    let mut full = usages(&fake)[4..].to_vec();
    full.sort();
    assert_eq!(full, ["alice-access", "bob-access"]);
    // Bob's card shows the next full round.
    app.background_round(4, Pool::Claude).await;
    let schedule = app.schedule(Pool::Claude).unwrap();
    assert_eq!((schedule.tick, schedule.full_every), (5, 3));
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    let bob = accounts.iter().find(|a| !a.is_current).unwrap();
    assert_eq!(bob.next_refresh_at, Some(schedule.at + 120));
}

#[tokio::test]
async fn a_new_refresh_interval_applies_to_the_round_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    app.background_round(1, Pool::Claude).await;
    assert_eq!(
        app.round_interval(Pool::Claude).as_secs(),
        REFRESH_SECONDS as u64
    );
    let mut preferences = app.preferences().unwrap();
    preferences.refresh.claude.seconds = 600;
    preferences.refresh.claude.full_every = 2;
    app.save_preferences(preferences.clone()).await.unwrap();
    let now = now_unix_seconds();
    let schedule = app.schedule(Pool::Claude).unwrap();
    assert_eq!(
        (schedule.interval, schedule.full_every, schedule.tick),
        (600, 2, 2)
    );
    assert!((now + 599..=now + 600).contains(&schedule.at));
    assert_eq!(app.round_interval(Pool::Claude).as_secs(), 600);
    // Saved and read back; other pools keep theirs.
    let saved = app.preferences().unwrap().refresh;
    assert_eq!(saved.claude.seconds, 600);
    assert_eq!(saved.codex, crate::RefreshPreferences::default().codex);
    assert_eq!(app.schedule(Pool::Codex), None);
    // Out of range, it is refused.
    preferences.refresh.claude.seconds = 1;
    assert!(app.save_preferences(preferences).await.is_err());
}

#[tokio::test]
async fn a_refresh_set_while_a_round_runs_schedules_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    // Set while the round asks for usage.
    let settings = app.paths.data_dir.join("settings.json");
    fake.usage_hooks.lock().unwrap().push(Box::new(move || {
        let mut value: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
        value["refresh"]["claude"] =
            json!({"seconds": 600, "fullEvery": 2, "adaptive": false, "backoff": true});
        fs::write(&settings, serde_json::to_vec(&value).unwrap()).unwrap();
    }));
    age(&app, REFRESH_SECONDS);
    let interval = app.background_round(1, Pool::Claude).await;
    assert!(fake.usage_hooks.lock().unwrap().is_empty());
    assert_eq!(interval.as_secs(), 600);
    let schedule = app.schedule(Pool::Claude).unwrap();
    assert_eq!((schedule.interval, schedule.full_every), (600, 2));
}

#[tokio::test]
async fn refreshing_the_pool_puts_the_next_round_off_by_an_interval() {
    use crate::refresh_policy::Schedule;
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let next = |accounts: Vec<AccountSummary>| accounts[0].next_refresh_at.unwrap();
    // A round ended a minute ago; refreshed now, Alice would miss the
    // next one and wait for the one after.
    let now = now_unix_seconds();
    let round = |at| Schedule {
        at,
        tick: 2,
        interval: REFRESH_SECONDS,
        full_every: 1,
    };
    app.set_schedule(Pool::Claude, round(now + REFRESH_SECONDS - 60));
    app.refresh_all(Pool::Claude).await.unwrap();
    let now = now_unix_seconds();
    assert!(next(app.accounts(Pool::Claude).await.unwrap()) > now + REFRESH_SECONDS);
    // Deferred, the next round asks for her an interval from now.
    app.defer_background_round(Pool::Claude);
    let schedule = app.schedule(Pool::Claude).unwrap();
    assert!((now..=now + 1).contains(&(schedule.at - REFRESH_SECONDS)));
    assert_eq!(schedule.tick, 2);
    assert_eq!(next(app.accounts(Pool::Claude).await.unwrap()), schedule.at);
    // A round under way schedules the next itself; the other pool stays.
    app.set_schedule(Pool::Claude, round(now - 1));
    app.defer_background_round(Pool::Claude);
    assert_eq!(app.schedule(Pool::Claude), Some(round(now - 1)));
    assert_eq!(app.schedule(Pool::Codex), None);
    let codex = Schedule {
        at: now + 5,
        tick: 3,
        interval: 10,
        full_every: 6,
    };
    app.set_schedule(Pool::Codex, codex);
    app.defer_background_round(Pool::Codex);
    let deferred = app.schedule(Pool::Codex).unwrap();
    assert!((now + 10..=now + 11).contains(&deferred.at));
    assert_eq!((deferred.tick, deferred.interval), (3, 10));
    assert_eq!(app.schedule(Pool::Claude), Some(round(now - 1)));
}

#[tokio::test]
async fn cards_show_when_the_background_refresh_asks_for_them_next() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _fake) = app(&dir).await;
    for name in ["carol", "bob", "alice"] {
        sign_in(&location, name, LATER);
        app.claude_import_current().await.unwrap();
    }
    let id = |name: &str| {
        let store = app.claude_load().unwrap();
        let email = format!("{name}@example.com");
        let account = store
            .accounts
            .iter()
            .find(|a| text(&a.own.account["emailAddress"]).as_deref() == Some(email.as_str()));
        account.unwrap().id.clone()
    };
    let (alice, bob, carol) = (id("alice"), id("bob"), id("carol"));
    // Nothing is scheduled before the first background round, which
    // asks no one: every account was asked at import.
    let next = |accounts: &[AccountSummary], id: &str| {
        accounts
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .next_refresh_at
    };
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(next(&accounts, &alice), None);
    app.background_round(1, Pool::Claude).await;
    let schedule = app.schedule(Pool::Claude).unwrap();
    assert_eq!((schedule.tick, schedule.interval), (2, REFRESH_SECONDS));
    // Inactive accounts have the same next round as the current one.
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(next(&accounts, &bob), Some(schedule.at));
    assert_eq!(app.next_full_refresh_at(Pool::Claude), Some(schedule.at));
    // A rate limit pause holds Bob; Carol waits for a manual refresh.
    let mut store = app.claude_load().unwrap();
    for account in &mut store.accounts {
        if account.id == bob {
            account.own.usage_retry_at = Some(schedule.at + 5 * REFRESH_SECONDS + 10);
        } else if account.id == carol {
            account.first_request_pending = true;
        }
    }
    app.claude_save(&store).unwrap();
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    // Paused accounts return on the first round after their pause.
    assert_eq!(next(&accounts, &alice), Some(schedule.at));
    assert_eq!(
        next(&accounts, &bob),
        Some(schedule.at + 6 * REFRESH_SECONDS)
    );
    assert_eq!(next(&accounts, &carol), None);
}

#[tokio::test]
async fn usage_rate_limits_pause_manual_and_background_refresh_until_backoff_expires() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.accounts(Pool::Claude).await.unwrap()[0].id.clone();
    queue_usage_rate_limit(&fake, json!({"error":{"message":"Rate limited"}}));
    let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
    assert!(error.contains("HTTP 429"));
    let until = app.claude_load().unwrap().accounts[0]
        .own
        .usage_retry_at
        .unwrap();
    assert!(until > now_unix_seconds());
    fake.usage
        .lock()
        .unwrap()
        .push((200, json!({"five_hour":{"utilization":42}})));
    assert_eq!(
        error,
        "Claude usage request failed: HTTP 429 Rate limited; retry paused for 5 minutes"
    );
    assert_eq!(
        app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
        error
    );
    // The background round passes the paused account by.
    age(&app, REFRESH_SECONDS);
    app.refresh_recurring(Pool::Claude, 0).await.unwrap();
    assert_eq!(fake.usage.lock().unwrap().len(), 1);
    let mut store = app.claude_load().unwrap();
    store.accounts[0].own.usage_retry_at = Some(now_unix_seconds() - 1);
    app.claude_save(&store).unwrap();
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    let stored = app.claude_load().unwrap();
    assert!(stored.accounts[0].own.usage_retry_at.is_none());
    assert!(stored.accounts[0].usage_error.is_none());
    assert!(fake.usage.lock().unwrap().is_empty());
}

#[tokio::test]
async fn without_the_slow_down_rate_limits_pause_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    // A pause under way lifts once the slow-down is turned off.
    queue_usage_rate_limit(&fake, json!({"error":{"message":"Rate limited"}}));
    app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
    assert!(app.claude_load().unwrap().accounts[0]
        .own
        .usage_retry_at
        .is_some());
    let mut preferences = app.preferences().unwrap();
    preferences.refresh.claude.backoff = false;
    app.save_preferences(preferences).await.unwrap();
    // A 429 now pauses nothing, nor counts toward the next pause.
    queue_usage_rate_limit(&fake, json!({"error":{"message":"Rate limited"}}));
    assert_eq!(
        app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
        "Claude usage request failed: HTTP 429 Rate limited"
    );
    let stored = app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(
        (stored.own.usage_retry_at, stored.own.rate_limits),
        (None, 0)
    );
    fake.usage
        .lock()
        .unwrap()
        .push((200, json!({"five_hour":{"utilization":42}})));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert!(fake.usage.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_cards_own_slow_down_applies_to_it_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let ids: Vec<String> = app
        .claude_load()
        .unwrap()
        .accounts
        .iter()
        .map(|a| a.id.clone())
        .collect();
    let (off, on) = (&ids[0], &ids[1]);
    let mut preferences = app.preferences().unwrap();
    preferences.refresh.cards.claude.insert(
        off.clone(),
        crate::refresh_policy::CardRefresh {
            adaptive: None,
            backoff: Some(false),
        },
    );
    app.save_preferences(preferences).await.unwrap();
    for id in [off, on] {
        queue_usage_rate_limit(&fake, json!({"error":{"message":"Rate limited"}}));
        app.refresh_usage(Pool::Claude, id).await.unwrap_err();
    }
    let store = app.claude_load().unwrap();
    let stored = |id: &String| store.accounts.iter().find(|a| &a.id == id).unwrap();
    assert_eq!(
        (stored(off).own.usage_retry_at, stored(off).own.rate_limits),
        (None, 0)
    );
    assert!(stored(on).own.usage_retry_at.is_some());
    assert_eq!(stored(on).own.rate_limits, 1);
    // The paused card waits without a request; the other asks at once.
    let asked = usages(&fake).len();
    app.refresh_usage(Pool::Claude, on).await.unwrap_err();
    assert_eq!(usages(&fake).len(), asked);
    app.refresh_usage(Pool::Claude, off).await.unwrap();
    assert_eq!(usages(&fake).len(), asked + 1);
}

#[tokio::test]
async fn token_rate_limits_pause_all_refreshes_and_recover_after_backoff() {
    for expires in [1, LATER] {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", expires);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        let credentials = fs::read(location.credentials_file()).unwrap();
        if expires == LATER {
            fake.usage.lock().unwrap().push((401, Value::Null));
        }
        *fake.refresh_error.lock().unwrap() = Some(TOKEN_REFRESH_RATE_LIMITED.into());
        let before = now_unix_seconds();
        let paused = format!("{TOKEN_REFRESH_RATE_LIMITED}; retry paused for 5 minutes");
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
            paused
        );
        let stored = app.claude_load().unwrap();
        let retry_at = stored.accounts[0].own.usage_retry_at.unwrap();
        assert!((before + 300..=now_unix_seconds() + 300).contains(&retry_at));
        assert_eq!(stored.accounts[0].document["refreshToken"], "alice-refresh");
        assert_eq!(fs::read(location.credentials_file()).unwrap(), credentials);
        let usage_calls = fake.usage_tokens.lock().unwrap().len();
        assert_eq!(fake.refreshed.lock().unwrap().len(), 1);
        // A persisted cooldown survives reopening the application.
        drop(app);
        let (app, _, _) = waiting_app(&dir);
        app.set_claude_network(fake.clone());
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
            paused
        );
        assert!(app.refresh_all(Pool::Claude).await.is_err());
        // The background round passes the paused account by.
        age(&app, REFRESH_SECONDS);
        app.refresh_recurring(Pool::Claude, 0).await.unwrap();
        assert_eq!(fake.refreshed.lock().unwrap().len(), 1);
        assert_eq!(fake.usage_tokens.lock().unwrap().len(), usage_calls);
        let mut store = app.claude_load().unwrap();
        store.accounts[0].own.usage_retry_at = Some(now_unix_seconds() - 1);
        app.claude_save(&store).unwrap();
        if expires == LATER {
            fake.usage.lock().unwrap().push((401, Value::Null));
        }
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        assert_eq!(fake.refreshed.lock().unwrap().len(), 2);
        let stored = app.claude_load().unwrap();
        assert!(stored.accounts[0].own.usage_retry_at.is_none());
        assert!(stored.accounts[0].usage_error.is_none());
        assert_eq!(stored.accounts[0].document["refreshToken"], "fresh-refresh");
    }
}

#[tokio::test]
async fn other_token_failures_are_not_classified_as_rate_limits() {
    for error in [SIGN_IN_EXPIRED, "Claude token refresh failed: HTTP 500"] {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        *fake.refresh_error.lock().unwrap() = Some(error.into());
        assert_eq!(
            app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
            error
        );
        assert!(app.claude_load().unwrap().accounts[0]
            .own
            .usage_retry_at
            .is_none());
    }
}

#[tokio::test]
async fn request_errors_preserve_the_cause_without_auth_headers() {
    // A truncated response produces a real transport error deterministically.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
            .await
            .unwrap();
    });
    let error = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{address}/"))
        .bearer_auth("synthetic-secret")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap_err();
    let top_level = error.to_string();
    let message = request_error(error);
    assert!(message.starts_with(&top_level));
    assert!(message.contains(" -> "), "{message}");
    assert!(!message.contains("synthetic-secret"));
    server.await.unwrap();
}

/// Serves one canned response per connection and returns each request's
/// head (lowercased) and body.
async fn token_server(
    responses: Vec<&'static str>,
) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/oauth/token", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut requests = vec![];
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut raw = vec![];
            let mut buffer = [0; 4096];
            let (head, body) = loop {
                let read = stream.read(&mut buffer).await.unwrap();
                raw.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                if let Some(end) = text.find("\r\n\r\n") {
                    let head = text[..end].to_lowercase();
                    let length: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse().unwrap());
                    if raw.len() >= end + 4 + length {
                        break (head, text[end + 4..end + 4 + length].to_owned());
                    }
                }
            };
            requests.push((head, serde_json::from_str(&body).unwrap()));
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    (url, server)
}

#[tokio::test]
async fn refresh_requests_match_claude_code_without_a_user_agent() {
    let json = |status: &str, body: &str| -> &'static str {
        Box::leak(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .into_boxed_str(),
            )
    };
    let (url, server) = token_server(vec![
            json("200 OK", r#"{"access_token":"new","refresh_token":"next","expires_in":28800}"#),
            json("400 Bad Request", r#"{"error":"invalid_grant","error_description":"Refresh token not found or invalid"}"#),
            json("429 Too Many Requests", r#"{"error":{"type":"rate_limit_error","message":"Rate limited. Please try again later."}}"#),
            json("500 Internal Server Error", "{}"),
            json("502 Bad Gateway", r#"{"error":{"message":"Claude account has no proxy route."}}"#),
        ])
        .await;
    let scope = "user:inference user:profile";
    let tokens = refresh_at(&url, "rt-1", scope, "direct://").await.unwrap();
    assert_eq!(tokens["access_token"], "new");
    assert_eq!(
        refresh_at(&url, "rt-2", scope, "direct://")
            .await
            .unwrap_err(),
        SIGN_IN_EXPIRED
    );
    assert_eq!(
        refresh_at(&url, "rt-3", scope, "direct://")
            .await
            .unwrap_err(),
        TOKEN_REFRESH_RATE_LIMITED
    );
    assert_eq!(
        refresh_at(&url, "rt-4", scope, "direct://")
            .await
            .unwrap_err(),
        "Claude token refresh failed: HTTP 500"
    );
    assert_eq!(
        refresh_at(&url, "rt-5", scope, "direct://")
            .await
            .unwrap_err(),
        "Claude token refresh failed: HTTP 502 Claude account has no proxy route."
    );
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 5);
    for (n, (head, body)) in requests.iter().enumerate() {
        assert!(head.starts_with("post /v1/oauth/token http/1.1"), "{head}");
        // The token endpoint refuses `claude-code/…` and `curl/…` agents.
        assert!(!head.contains("\r\nuser-agent:"), "{head}");
        assert!(
            head.contains("\r\ncontent-type: application/json"),
            "{head}"
        );
        assert!(!head.contains("authorization:"), "{head}");
        assert!(!head.contains("anthropic-beta:"), "{head}");
        assert_eq!(
            *body,
            json!({
                "grant_type": "refresh_token",
                "refresh_token": format!("rt-{}", n + 1),
                "client_id": crate::claude_api::CLIENT,
                "scope": scope,
            })
        );
    }
}

#[tokio::test]
async fn a_refresh_sends_the_login_scopes_and_records_both_expiries() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _, fake) = waiting_app(&dir);
    let (mut oauth, account) = login("bob", 1);
    oauth["scopes"] = json!(["user:inference", "user:profile"]);
    app.claude_import_json(&json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string())
        .await
        .unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(
        *fake.refresh_scopes.lock().unwrap(),
        ["user:inference user:profile"]
    );
    // A login without scopes asks for the default ones.
    let (expiring, other) = login("carol", 1);
    app.claude_import_json(&json!({"claudeAiOauth": expiring, "oauthAccount": other}).to_string())
        .await
        .unwrap();
    let carol = app
        .claude_load()
        .unwrap()
        .accounts
        .into_iter()
        .find(|a| a.label == "carol@example.com")
        .unwrap()
        .id;
    app.release_new_accounts(Pool::Claude, Some(&carol))
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &carol).await.unwrap();
    assert_eq!(
        fake.refresh_scopes.lock().unwrap()[1],
        crate::claude_api::SCOPES
    );
    let mut renewed = json!({});
    let now = 1_000_000;
    apply_tokens(
            &mut renewed,
            &json!({"access_token": "a", "refresh_token": "r", "expires_in": 28800, "refresh_token_expires_in": 2_592_000, "scope": "user:inference user:profile"}),
            now,
        )
        .unwrap();
    assert_eq!(renewed["expiresAt"], now + 28_800_000);
    assert_eq!(renewed["refreshTokenExpiresAt"], now + 2_592_000_000);
    assert_eq!(renewed["scopes"], json!(["user:inference", "user:profile"]));
    // A response without it leaves the recorded expiry.
    renewed["refreshTokenExpiresAt"] = 5.into();
    apply_tokens(&mut renewed, &json!({"access_token": "b"}), now).unwrap();
    assert_eq!(renewed["refreshTokenExpiresAt"], 5);
    assert_eq!(renewed["refreshToken"], "r");
    // A lifetime that is no number leaves the expiry; one too long for
    // the clock is the latest it can say.
    renewed["expiresAt"] = 7.into();
    apply_tokens(
        &mut renewed,
        &json!({"access_token": "c", "expires_in": "NaN", "refresh_token_expires_in": 1e300}),
        now,
    )
    .unwrap();
    assert_eq!(renewed["expiresAt"], 7);
    assert_eq!(renewed["refreshTokenExpiresAt"], i64::MAX);
    assert_eq!(crate::claude_api::number(&json!("inf")), None);
    assert_eq!(crate::claude_api::number(&json!("2.5")), Some(2.5));
}

/// Claude's real token endpoint, through `COPOOL_LIVE_CLAUDE_PROXY` (or
/// the system proxy), with a refresh token that cannot exist: a request
/// it accepts is refused as `invalid_grant`, one it filters gets HTTP 429.
#[tokio::test]
#[ignore = "contacts platform.claude.com"]
async fn the_live_token_endpoint_accepts_the_refresh_request() {
    let proxy = std::env::var("COPOOL_LIVE_CLAUDE_PROXY").unwrap_or_default();
    let error = refresh_at(
        crate::claude_api::TOKEN_URL,
        "sk-ant-ort01-copool-invalid-probe",
        crate::claude_api::SCOPES,
        &proxy,
    )
    .await
    .unwrap_err();
    assert_eq!(error, SIGN_IN_EXPIRED);
}

fn agents(fake: &Fake) -> Vec<String> {
    fake.usage_agents.lock().unwrap().clone()
}

#[tokio::test]
async fn fallback_updates_windows_and_keeps_resets_until_confirmed_again() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    let grants =
        json!([{"resets_left":1,"ends_at":"2099-01-01T00:00:00Z","gives_back_weekly":true}]);
    fake.usage.lock().unwrap().push((
        200,
        json!({"cedar_ember":{"eligible":true,"grants":grants}}),
    ));
    app.claude_import_current().await.unwrap();
    let stored = || app.claude_load().unwrap().accounts[0].clone();
    let id = stored().id;
    let first = stored().usage.unwrap();
    // First 429 on the CLI agent falls back to the legacy agent and pauses
    // the CLI agent for this account so the next poll does not hit 429 again.
    fake.usage.lock().unwrap().extend([
        (429, Value::Null),
        (
            200,
            json!({"five_hour":{"utilization":41},"cedar_ember":{"eligible":false,"ineligible_reason":"surface"}}),
        ),
        (
            200,
            json!({"five_hour":{"utilization":42},"cedar_ember":{"eligible":false,"ineligible_reason":"surface"}}),
        ),
    ]);
    for used in [41, 42] {
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        let usage = stored().usage.unwrap();
        assert_eq!(usage.five_hour.unwrap().used_percent, f64::from(used));
        assert_eq!(usage.remaining_reset_count, Some(1));
        assert_eq!(usage.reset_grants[0].kind, ResetKind::Fable);
        assert_eq!(usage.reset_expiries, first.reset_expiries);
        assert_eq!(usage.resets_last_confirmed_at, Some(first.fetched_at));
    }
    // Once the CLI agent's pause expires, the next refresh probes it again
    // and confirms the updated reset inventory.
    app.claude
        .usage_agent_retry_at
        .lock()
        .unwrap()
        .insert(id.clone(), now_unix_seconds() - 1);
    fake.usage
        .lock()
        .unwrap()
        .push((200, json!({"cedar_ember":{"eligible":true,"grants":[]}})));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    let usage = stored().usage.unwrap();
    assert_eq!(usage.remaining_reset_count, Some(0));
    assert!(usage.reset_grants.is_empty());
    assert_eq!(usage.resets_last_confirmed_at, None);
    let (cli, code) = (USAGE_USER_AGENT, USER_AGENT);
    assert_eq!(agents(&fake), [cli, cli, code, code, cli]);

    // Turning the card's backoff off stops pausing the CLI agent.
    let mut preferences = app.preferences().unwrap();
    preferences.refresh.cards.claude.insert(
        id.clone(),
        crate::refresh_policy::CardRefresh {
            adaptive: None,
            backoff: Some(false),
        },
    );
    app.save_preferences(preferences).await.unwrap();
    let start = agents(&fake).len();
    fake.usage.lock().unwrap().extend([
        (429, Value::Null),
        (200, json!({"five_hour":{"utilization":43}})),
        (200, json!({"five_hour":{"utilization":43}})),
    ]);
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(agents(&fake)[start..], [cli, code, cli]);
}

#[tokio::test]
async fn clock_jitter_and_fallback_metadata_do_not_speed_up_an_idle_card() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    let grants = json!([{"resets_left":1,"ends_at":"2099-01-01T00:00:00Z","gives_back_weekly":false,"clears":["five_hour","seven_day"]}]);
    *fake.usage_clock_offset.lock().unwrap() = Some(0);
    fake.usage.lock().unwrap().push((
        200,
        json!({
            "five_hour": {"utilization": 0.0, "resets_at": null},
            "seven_day": {"utilization": 100.0, "resets_at": "2099-01-01T00:00:00Z"},
            "cedar_ember": {"eligible": true, "grants": grants}
        }),
    ));
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();

    // Clock offset flips 0 -> -1, resets_at jitters by 1 second, and a 429
    // fallback sets resetsLastConfirmedAt: none of these is a usage change.
    *fake.usage_clock_offset.lock().unwrap() = Some(-1);
    fake.usage.lock().unwrap().extend([
        (429, Value::Null),
        (
            200,
            json!({
                "five_hour": {"utilization": 0.0, "resets_at": null},
                "seven_day": {"utilization": 100.0, "resets_at": "2098-12-31T23:59:59Z"},
                "cedar_ember": {"eligible": false, "ineligible_reason": "surface"}
            }),
        ),
    ]);
    age(&app, REFRESH_SECONDS);
    app.background_round(1, Pool::Claude).await;
    assert_eq!(app.fast_wait(Pool::Claude), None);
    assert!(app.speedup(Pool::Claude, &id).is_none());
}

#[tokio::test]
async fn an_idle_noncurrent_card_returns_to_six_minute_rounds_despite_clock_jitter() {
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    let usage = |used, reset| {
        (
            200,
            json!({"five_hour": {"utilization": used, "resets_at": reset}}),
        )
    };
    sign_in(&location, "alice", LATER);
    fake.usage
        .lock()
        .unwrap()
        .push(usage(20, "2099-01-01T00:00:00Z"));
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    sign_in(&location, "bob", LATER);
    fake.usage
        .lock()
        .unwrap()
        .push(usage(25, "2099-01-01T00:00:00Z"));
    app.claude_import_current().await.unwrap();
    let mut preferences = app.preferences().unwrap();
    preferences.refresh.claude.seconds = 120;
    preferences.refresh.claude.full_every = 3;
    app.save_preferences(preferences).await.unwrap();

    // A genuine increase starts 30-second polling for the noncurrent card.
    fake.usage.lock().unwrap().extend([
        usage(25, "2099-01-01T00:00:00Z"),
        usage(25, "2099-01-01T00:00:00Z"),
    ]);
    age(&app, 360);
    app.background_round(3, Pool::Claude).await;
    assert_eq!(app.fast_wait(Pool::Claude), Some(Duration::from_secs(30)));
    let requests = usages(&fake).len();
    // Two unchanged readings exit fast mode even though clock data jitters.
    for (round, offset, reset) in [
        (0, -1, "2098-12-31T23:59:59Z"),
        (1, 0, "2099-01-01T00:00:00Z"),
    ] {
        *fake.usage_clock_offset.lock().unwrap() = Some(offset);
        fake.usage.lock().unwrap().push(usage(25, reset));
        age(&app, 30);
        app.fast_round(Pool::Claude).await;
        assert_eq!(usages(&fake).len(), requests + round + 1);
        assert_eq!(usages(&fake).last().unwrap(), "alice-access");
        assert_eq!(
            app.fast_wait(Pool::Claude),
            (round == 0).then_some(Duration::from_secs(30))
        );
    }
    let card = app
        .accounts(Pool::Claude)
        .await
        .unwrap()
        .into_iter()
        .find(|a| a.id == id)
        .unwrap();
    assert!(!card.is_current);
    assert_eq!(card.refresh.unwrap().seconds, 360);
    let schedule = app.schedule(Pool::Claude).unwrap();
    assert_eq!(card.next_refresh_at, Some(schedule.at + 240));
    assert!(app.speedup(Pool::Claude, &id).is_none());

    // An extra fast tick sends nothing. Intermediate rounds ask only Bob.
    let requests = usages(&fake).len();
    age(&app, 120);
    app.fast_round(Pool::Claude).await;
    assert_eq!(usages(&fake).len(), requests);
    for tick in [4, 5] {
        fake.usage
            .lock()
            .unwrap()
            .push(usage(25, "2099-01-01T00:00:00Z"));
        age(&app, 120);
        app.background_round(tick, Pool::Claude).await;
        assert_eq!(app.fast_wait(Pool::Claude), None);
    }
    assert_eq!(usages(&fake).len(), requests + 2);
    // The next full round asks Alice again without restarting fast polling.
    age(&app, 120);
    fake.usage.lock().unwrap().extend([
        usage(25, "2098-12-31T23:59:59Z"),
        usage(25, "2098-12-31T23:59:59Z"),
    ]);
    app.background_round(6, Pool::Claude).await;
    assert_eq!(usages(&fake).len(), requests + 4);
    assert_eq!(app.fast_wait(Pool::Claude), None);
}

#[tokio::test]
async fn primary_agent_cooldown_honors_retry_after_and_fallback_can_pause_all_usage() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    *fake.usage_retry_after.lock().unwrap() = Some(5400);
    fake.usage.lock().unwrap().push((429, Value::Null));
    let before = now_unix_seconds();
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    let until = app.claude.usage_agent_retry_at.lock().unwrap()[&id];
    assert!((before + 5400..=now_unix_seconds() + 5400).contains(&until));
    assert!(app.claude_load().unwrap().accounts[0]
        .own
        .usage_retry_at
        .is_none());

    // A later 429 from the fallback still invokes the account-wide backoff.
    *fake.usage_retry_after.lock().unwrap() = None;
    let start = agents(&fake).len();
    fake.usage.lock().unwrap().push((429, Value::Null));
    let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
    assert!(error.ends_with("retry paused for 5 minutes"));
    assert_eq!(agents(&fake)[start..], [USER_AGENT]);
    assert_eq!(
        app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
        error
    );
    assert_eq!(agents(&fake).len(), start + 1);
}

#[tokio::test]
async fn a_refusal_does_not_affect_another_account_on_the_same_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    let id = |user: &str| {
        app.claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.label == format!("{user}@example.com"))
            .unwrap()
            .id
    };
    app.save_account_proxy(Pool::Claude, &id("alice"), "http://127.0.0.1:7001")
        .await
        .unwrap();
    app.save_account_proxy(Pool::Claude, &id("bob"), "http://127.0.0.1:7001")
        .await
        .unwrap();
    let (cli, code) = (USAGE_USER_AGENT, USER_AGENT);
    let start = agents(&fake).len();
    fake.usage.lock().unwrap().push((429, Value::Null));
    app.refresh_usage(Pool::Claude, &id("alice")).await.unwrap();
    app.refresh_usage(Pool::Claude, &id("bob")).await.unwrap();
    assert_eq!(agents(&fake)[start..], [cli, code, cli]);
    assert_eq!(
        fake.usage_proxies.lock().unwrap()[start..],
        [
            "http://127.0.0.1:7001",
            "http://127.0.0.1:7001",
            "http://127.0.0.1:7001"
        ]
    );
    // Refused both ways: a rate limit, counted and paused once.
    fake.usage.lock().unwrap().push((429, Value::Null));
    fake.usage
        .lock()
        .unwrap()
        .push((429, json!({"error": {"message": "Rate limited"}})));
    *fake.usage_retry_after.lock().unwrap() = Some(5400);
    let error = app
        .refresh_usage(Pool::Claude, &id("bob"))
        .await
        .unwrap_err();
    assert_eq!(agents(&fake)[start + 3..], [cli, code]);
    assert_eq!(
        error,
        "Claude usage request failed: HTTP 429 Rate limited; retry paused for 90 minutes"
    );
    let bob = app
        .claude_load()
        .unwrap()
        .accounts
        .into_iter()
        .find(|a| a.id == id("bob"))
        .unwrap();
    assert_eq!(bob.own.rate_limits, 1);
}

#[tokio::test]
async fn the_usage_retry_after_a_renewal_falls_back_as_well() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    fake.usage.lock().unwrap().push((401, Value::Null));
    fake.usage.lock().unwrap().push((429, Value::Null));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(
        agents(&fake),
        [USAGE_USER_AGENT, USAGE_USER_AGENT, USER_AGENT]
    );
    assert_eq!(
        usages(&fake),
        ["alice-access", "fresh-access", "fresh-access"]
    );
}

/// Copool's Keychain item, in memory.
#[derive(Default)]
struct MemoryVault {
    text: Mutex<Option<String>>,
    unreadable: std::sync::atomic::AtomicBool,
    unwritable: std::sync::atomic::AtomicBool,
    /// Writes report success without storing anything.
    lost_writes: std::sync::atomic::AtomicBool,
    /// Reads report the item missing, as the Keychain can to a process
    /// it hides an item from.
    hidden: std::sync::atomic::AtomicBool,
    reads: Mutex<Vec<KeychainAccess>>,
    writes: Mutex<usize>,
}
impl Vault for MemoryVault {
    fn read(&self, access: KeychainAccess) -> Result<Option<String>> {
        self.reads.lock().unwrap().push(access);
        if self.unreadable.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("The user denied access".into());
        }
        if self.hidden.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(self.text.lock().unwrap().clone())
    }
    fn write(&self, text: &str, _: KeychainAccess) -> Result<()> {
        if self.unwritable.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("The user denied access".into());
        }
        *self.writes.lock().unwrap() += 1;
        if !self.lost_writes.load(std::sync::atomic::Ordering::SeqCst) {
            *self.text.lock().unwrap() = Some(text.into());
        }
        Ok(())
    }
}
impl MemoryVault {
    fn secrets(&self) -> Value {
        serde_json::from_str(self.text.lock().unwrap().as_deref().unwrap_or("{}")).unwrap()
    }
    fn writes(&self) -> usize {
        *self.writes.lock().unwrap()
    }
    fn reads(&self) -> Vec<KeychainAccess> {
        self.reads.lock().unwrap().clone()
    }
    fn set(&self, flag: &std::sync::atomic::AtomicBool, value: bool) {
        flag.store(value, std::sync::atomic::Ordering::SeqCst);
    }
}
fn vault_app(
    dir: &tempfile::TempDir,
    vault: &Arc<MemoryVault>,
) -> (Application, Location, Arc<Fake>) {
    let (app, location, fake) = waiting_app(dir);
    app.set_claude_vault(vault.clone());
    (app, location, fake)
}
fn store_text(app: &Application) -> String {
    fs::read_to_string(app.claude_store_path()).unwrap()
}
fn assert_no_tokens(app: &Application, tokens: &[&str]) {
    let text = store_text(app);
    for token in tokens {
        assert!(!text.contains(token), "{token} in the store");
    }
}
fn export(oauth: &Value, account: &Value) -> String {
    json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string()
}
use crate::pool_login::claude::same_oauth_generation;
use KeychainAccess::{Background, Interactive};

#[tokio::test]
async fn the_pool_tokens_live_in_the_vault_and_the_store_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let (app, _, _) = vault_app(&dir, &vault);
    let (oauth, account) = login("bob", LATER);
    app.claude_import_json(&export(&oauth, &account))
        .await
        .unwrap();
    let (mut newer, _) = login("bob", LATER + 1);
    newer["accessToken"] = "bob-newer".into();
    newer["refreshToken"] = "bob-newer-refresh".into();
    app.claude_import_json(&export(&newer, &account))
        .await
        .unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    assert_no_tokens(&app, &["bob-access", "bob-refresh", "bob-newer"]);
    let file: Value = serde_json::from_str(&store_text(&app)).unwrap();
    let saved = &file["accounts"][0];
    assert_eq!(
        saved["document"],
        json!({"expiresAt": LATER + 1, "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"})
    );
    assert_eq!(saved["label"], "bob@example.com");
    assert_eq!(saved["firstRequestPending"], true);
    assert_eq!(
        vault.secrets(),
        json!({id.clone(): {"accessToken": "bob-newer", "refreshToken": "bob-newer-refresh"}})
    );
    let loaded = app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(loaded.document, newer);
    // Read once, asking the user; written, and read back, as tokens change.
    assert_eq!(vault.reads(), [Interactive, Background, Background]);
    assert_eq!(vault.writes(), 2);
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(vault.writes(), 2);
    assert_eq!(vault.reads().len(), 3);
    // Reopened, the tokens come back from the vault without asking.
    drop(app);
    let (app, _, fake) = vault_app(&dir, &vault);
    assert_eq!(app.accounts(Pool::Claude).await.unwrap().len(), 1);
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(usages(&fake), ["bob-newer"]);
    assert_eq!(vault.reads()[3..], [Background]);
}

#[tokio::test]
async fn tokens_left_in_the_store_move_to_the_vault_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    assert!(store_text(&app).contains("alice-refresh"));
    let before = app.claude_load().unwrap();
    drop(app);
    let vault = Arc::new(MemoryVault::default());
    let (app, _, _) = vault_app(&dir, &vault);
    app.claude_authorize_at_startup().await.unwrap();
    assert_eq!(vault.reads(), [Interactive, Background]);
    assert_no_tokens(&app, &["alice-access", "alice-refresh"]);
    let id = &before.accounts[0].id;
    assert_eq!(vault.secrets()[id]["refreshToken"], "alice-refresh");
    let after = app.claude_load().unwrap();
    assert_eq!(after.accounts[0].document, before.accounts[0].document);
    assert_eq!(after.current_account_id, before.current_account_id);
    assert!(after.accounts[0].first_request_pending);
}

#[tokio::test]
async fn an_unreadable_vault_loses_no_tokens_and_contacts_no_one() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let (app, location, _) = vault_app(&dir, &vault);
    let (oauth, account) = login("bob", LATER);
    app.claude_import_json(&export(&oauth, &account))
        .await
        .unwrap();
    let bob = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&bob))
        .await
        .unwrap();
    let held = vault.secrets();
    drop(app);
    // Reopened without access to the vault (a new build not yet allowed).
    vault.set(&vault.unreadable, true);
    let (app, _, fake) = vault_app(&dir, &vault);
    let cards = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(cards.len(), 1);
    // The card shows its tokens are out of reach, as its error.
    let locked = |error: &Option<String>| {
        error
            .as_deref()
            .is_some_and(|e| e.starts_with(VAULT_LOCKED) && e.contains("denied"))
    };
    assert!(locked(&cards[0].usage_error));
    assert!(app
        .refresh_usage(Pool::Claude, &bob)
        .await
        .unwrap_err()
        .starts_with(VAULT_LOCKED));
    assert!(usages(&fake).is_empty());
    assert!(refreshes(&fake).is_empty());
    // Another import keeps its tokens in the store; the vault is untouched.
    let (carol, carol_account) = login("carol", LATER);
    app.claude_import_json(&export(&carol, &carol_account))
        .await
        .unwrap();
    assert!(store_text(&app).contains("carol-refresh"));
    assert_no_tokens(&app, &["bob-access", "bob-refresh"]);
    assert_eq!(vault.secrets(), held);
    assert_eq!(vault.writes(), 1);
    // Claude Code is never handed a login without its tokens.
    assert!(app
        .switch_account(Pool::Claude, &bob)
        .await
        .unwrap_err()
        .starts_with(VAULT_LOCKED));
    assert!(!location.credentials_file().exists());
    // The account whose tokens stayed in the store is not locked.
    let cards = app.accounts(Pool::Claude).await.unwrap();
    let card = |label: &str| cards.iter().find(|c| c.label == label).unwrap();
    let account_locked = |label: &str| locked(&card(label).usage_error);
    assert!(account_locked("bob@example.com"));
    assert!(!account_locked("carol@example.com"));
    // A user action, such as a manual refresh, asks again; once allowed,
    // every token is there and moves to the vault.
    vault.set(&vault.unreadable, false);
    let asked = vault.reads().len();
    app.claude_authorize().await.unwrap();
    assert_eq!(vault.reads()[asked..].first(), Some(&Interactive));
    assert!(app
        .accounts(Pool::Claude)
        .await
        .unwrap()
        .iter()
        .all(|card| !locked(&card.usage_error)));
    let store = app.claude_load().unwrap();
    let tokens: Vec<_> = store
        .accounts
        .iter()
        .map(|a| a.document["refreshToken"].clone())
        .collect();
    assert_eq!(tokens, [json!("bob-refresh"), json!("carol-refresh")]);
    app.claude_authorize_at_startup().await.unwrap();
    assert_no_tokens(&app, &["bob-refresh", "carol-refresh"]);
    assert_eq!(vault.secrets().as_object().unwrap().len(), 2);
    app.refresh_usage(Pool::Claude, &bob).await.unwrap();
    assert_eq!(usages(&fake), ["bob-access"]);
}

#[tokio::test]
async fn a_vault_that_does_not_take_a_write_leaves_the_tokens_in_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let (app, _, _) = vault_app(&dir, &vault);
    // Refused: the store keeps them.
    vault.set(&vault.unwritable, true);
    let (oauth, account) = login("bob", LATER);
    app.claude_import_json(&export(&oauth, &account))
        .await
        .unwrap();
    assert!(store_text(&app).contains("bob-refresh"));
    assert_eq!(vault.secrets(), json!({}));
    assert_eq!(app.claude_load().unwrap().accounts[0].document, oauth);
    // Reported done but not stored: the read back tells, the store keeps them.
    vault.set(&vault.unwritable, false);
    vault.set(&vault.lost_writes, true);
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    assert!(vault.writes() >= 1);
    assert!(store_text(&app).contains("bob-refresh"));
    assert_eq!(vault.secrets(), json!({}));
    // The next save that sticks moves them.
    vault.set(&vault.lost_writes, false);
    app.save_account_proxy(Pool::Claude, &id, "http://127.0.0.1:7001")
        .await
        .unwrap();
    assert_no_tokens(&app, &["bob-access", "bob-refresh"]);
    assert_eq!(vault.secrets()[&id]["refreshToken"], "bob-refresh");
}

#[tokio::test]
async fn vault_contents_that_cannot_be_read_as_tokens_are_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    *vault.text.lock().unwrap() = Some("not the pool".into());
    let (app, _, _) = vault_app(&dir, &vault);
    let (oauth, account) = login("bob", LATER);
    app.claude_import_json(&export(&oauth, &account))
        .await
        .unwrap();
    assert!(store_text(&app).contains("bob-refresh"));
    assert_eq!(vault.text.lock().unwrap().as_deref(), Some("not the pool"));
    assert_eq!(vault.writes(), 0);
}

#[tokio::test]
async fn renewals_switches_and_deletions_go_through_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let (app, location, fake) = vault_app(&dir, &vault);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let (oauth, account) = login("bob", 1);
    app.claude_import_json(&export(&oauth, &account))
        .await
        .unwrap();
    let ids = |user: &str| {
        app.claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.label == format!("{user}@example.com"))
            .unwrap()
            .id
    };
    let (alice, bob) = (ids("alice"), ids("bob"));
    app.release_new_accounts(Pool::Claude, None).await.unwrap();
    // A renewal rotates the tokens in the vault, never in the store.
    app.refresh_usage(Pool::Claude, &bob).await.unwrap();
    assert_eq!(refreshes(&fake), ["bob-refresh"]);
    assert_eq!(vault.secrets()[&bob]["refreshToken"], "fresh-refresh");
    assert_no_tokens(&app, &["fresh-access", "fresh-refresh", "alice-refresh"]);
    // Renewed while the vault refuses writes: the store's newer tokens
    // win over the vault's until they can be moved.
    vault.set(&vault.unwritable, true);
    fake.usage.lock().unwrap().push((401, Value::Null));
    *fake.refresh_response.lock().unwrap() = Some(
        json!({"access_token": "next-access", "refresh_token": "next-refresh", "expires_in": 3600}),
    );
    app.refresh_usage(Pool::Claude, &bob).await.unwrap();
    assert_eq!(refreshes(&fake), ["bob-refresh", "fresh-refresh"]);
    assert!(store_text(&app).contains("next-refresh"));
    assert_eq!(vault.secrets()[&bob]["refreshToken"], "fresh-refresh");
    let rotated = app.claude_load().unwrap();
    let held = rotated.accounts.iter().find(|a| a.id == bob).unwrap();
    assert_eq!(held.document["refreshToken"], "next-refresh");
    vault.set(&vault.unwritable, false);
    // A switch hands Claude Code the tokens from the vault.
    app.switch_account(Pool::Claude, &bob).await.unwrap();
    assert_eq!(live_access(&location), "next-access");
    assert_no_tokens(&app, &["next-access", "next-refresh", "alice-access"]);
    assert_eq!(vault.secrets()[&bob]["refreshToken"], "next-refresh");
    assert_eq!(vault.secrets()[&alice]["refreshToken"], "alice-refresh");
    // A deleted account leaves the vault too.
    app.delete_account(Pool::Claude, &alice).await.unwrap();
    assert_eq!(
        vault
            .secrets()
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        [&bob]
    );
}

#[tokio::test]
async fn claude_codes_login_serves_its_account_while_the_vault_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let (app, location, _) = vault_app(&dir, &vault);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let (bob, bob_account) = login("bob", LATER);
    app.claude_import_json(&export(&bob, &bob_account))
        .await
        .unwrap();
    // The pool's alice tokens are not the ones Claude Code holds.
    let store = app.claude_load().unwrap();
    let alice = store.accounts[0].id.clone();
    drop(app);
    vault.set(&vault.unreadable, true);
    let (app, _, fake) = vault_app(&dir, &vault);
    // Claude Code renewed its login meanwhile.
    external_update(location.clone(), "alice", "alice-renewed")();
    app.accounts(Pool::Claude).await.unwrap();
    let account = app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(account.document["accessToken"], "alice-renewed");
    app.release_new_accounts(Pool::Claude, Some(&alice))
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &alice).await.unwrap();
    assert_eq!(usages(&fake), ["alice-renewed"]);
    // Readable again: the store's newer alice tokens win over the vault's
    // older ones, and bob's come from the vault.
    vault.set(&vault.unreadable, false);
    assert_eq!(vault.secrets()[&alice]["accessToken"], "alice-access");
    let store = app.claude_load().unwrap();
    assert_eq!(store.accounts[0].document["accessToken"], "alice-renewed");
    assert_eq!(store.accounts[1].document["accessToken"], "bob-access");
    app.claude_authorize_at_startup().await.unwrap();
    assert_eq!(vault.secrets()[&alice]["accessToken"], "alice-renewed");
    assert_no_tokens(&app, &["alice-renewed", "bob-access"]);
}

#[tokio::test]
async fn a_store_read_before_the_vault_opened_keeps_its_tokens_there() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let (app, _, _) = vault_app(&dir, &vault);
    let (oauth, account) = login("bob", LATER);
    app.claude_import_json(&export(&oauth, &account))
        .await
        .unwrap();
    let held = vault.secrets();
    drop(app);
    vault.set(&vault.unreadable, true);
    let (app, _, _) = vault_app(&dir, &vault);
    // Read while the vault was closed, saved after it opened.
    let stale = app.claude_load().unwrap();
    assert!(!holds_tokens(&stale.accounts[0]));
    vault.set(&vault.unreadable, false);
    app.claude_load().unwrap();
    app.claude_save(&stale).unwrap();
    assert_eq!(vault.secrets(), held);
    assert_eq!(app.claude_load().unwrap().accounts[0].document, oauth);
}

#[tokio::test]
async fn a_login_claude_code_reads_from_its_file_is_renewed_back_into_it() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = file_login_app(&dir);
    sign_in(&location, "alice", 1);
    app.claude_import_current().await.unwrap();
    let alice = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&alice))
        .await
        .unwrap();
    // By default renewed, and the renewal goes back to the file it was
    // read from, never to a new Keychain item.
    app.refresh_usage(Pool::Claude, &alice).await.unwrap();
    assert_eq!(refreshes(&fake), ["alice-refresh"]);
    assert_eq!(live_access(&location), "fresh-access");
    let document: Value =
        serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
    assert_eq!(document["mcpOAuth"]["kept"], true);
    // Renewal turned off: the file is followed, not renewed nor written.
    app.set_renewal(Pool::Claude, &alice, "never")
        .await
        .unwrap();
    external_update(location.clone(), "alice", "alice-synced")();
    let synced = fs::read(location.credentials_file()).unwrap();
    fake.usage.lock().unwrap().push((401, Value::Null));
    assert_eq!(
        app.refresh_usage(Pool::Claude, &alice).await.unwrap_err(),
        RENEWAL_OFF
    );
    assert_eq!(refreshes(&fake), ["alice-refresh"]);
    assert_eq!(fs::read(location.credentials_file()).unwrap(), synced);
    assert_eq!(
        app.claude_load().unwrap().accounts[0].document["accessToken"],
        "alice-synced"
    );
}

/// A pool whose current login Claude Code reads from its file, with
/// Copool's tokens in memory.
fn file_login_app(dir: &tempfile::TempDir) -> (Application, Location, Arc<Fake>) {
    let (app, files, fake) = waiting_app(dir);
    let location = Location {
        keychain: Some(format!(
            "io.github.copool.rust.test-absent-{}",
            std::process::id()
        )),
        ..files
    };
    app.set_claude_location(location.clone());
    app.set_claude_vault(Arc::new(MemoryVault::default()));
    (app, location, fake)
}
async fn renewals(app: &Application) -> Vec<(String, Option<String>)> {
    app.accounts(Pool::Claude)
        .await
        .unwrap()
        .into_iter()
        .map(|a| (a.label, a.renewal))
        .collect()
}

#[tokio::test]
async fn the_renewal_choice_holds_back_or_allows_renewal() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = file_login_app(&dir);
    sign_in(&location, "alice", 1);
    app.claude_import_current().await.unwrap();
    let (bob, account) = login("bob", 1);
    app.claude_import_json(&export(&bob, &account))
        .await
        .unwrap();
    app.release_new_accounts(Pool::Claude, None).await.unwrap();
    let id = |user: &str| {
        app.claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.label == format!("{user}@example.com"))
            .unwrap()
            .id
    };
    let (alice, bob) = (id("alice"), id("bob"));
    // By default every account, the file login included, is renewed.
    let mut cards = renewals(&app).await;
    cards.sort();
    assert_eq!(
        cards,
        [
            ("alice@example.com".to_string(), Some("always".to_string())),
            ("bob@example.com".to_string(), Some("always".to_string())),
        ]
    );
    // Never: an expired account is reported, not renewed.
    app.set_renewal(Pool::Claude, &bob, "never").await.unwrap();
    assert_eq!(
        app.refresh_usage(Pool::Claude, &bob).await.unwrap_err(),
        RENEWAL_OFF
    );
    assert!(refreshes(&fake).is_empty());
    let card = |label: &str, cards: Vec<(String, Option<String>)>| {
        cards.into_iter().find(|c| c.0 == label).unwrap().1
    };
    assert_eq!(
        card("bob@example.com", renewals(&app).await).as_deref(),
        Some("never")
    );
    // Always, chosen or by default, renews the file login into its file.
    app.set_renewal(Pool::Claude, &alice, "always")
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &alice).await.unwrap();
    assert_eq!(refreshes(&fake), ["alice-refresh"]);
    assert_eq!(live_access(&location), "fresh-access");
    assert_eq!(
        card("alice@example.com", renewals(&app).await).as_deref(),
        Some("always")
    );
    // The choice is stored; an unknown one or account is refused.
    let saved: Value = serde_json::from_str(&store_text(&app)).unwrap();
    let choices: Vec<_> = saved["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["renewal"].clone())
        .collect();
    assert!(choices.contains(&json!("always")) && choices.contains(&json!("never")));
    assert!(app
        .set_renewal(Pool::Claude, &bob, "sometimes")
        .await
        .is_err());
    assert!(app
        .set_renewal(Pool::Claude, "missing", "always")
        .await
        .is_err());
}

#[tokio::test]
async fn renew_now_renews_without_asking_usage_unless_the_account_failed() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = file_login_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let (bob, account) = login("bob", 1);
    app.claude_import_json(&export(&bob, &account))
        .await
        .unwrap();
    app.release_new_accounts(Pool::Claude, None).await.unwrap();
    let id = |user: &str| {
        app.claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.label == format!("{user}@example.com"))
            .unwrap()
            .id
    };
    let (alice, bob) = (id("alice"), id("bob"));
    let renewal = |cards: Vec<(String, Option<String>)>, label: &str| {
        cards.into_iter().find(|c| c.0 == label).unwrap().1
    };
    // An unexpired login is not renewed by a refresh.
    app.refresh_usage(Pool::Claude, &alice).await.unwrap();
    assert!(refreshes(&fake).is_empty());
    let asked = usages(&fake).len();
    // Renew now renews it even with renewal off, writes it back to
    // Claude Code's login, asks no usage and leaves the choice as it was.
    app.set_renewal(Pool::Claude, &alice, "never")
        .await
        .unwrap();
    app.renew_now(Pool::Claude, &alice).await.unwrap();
    assert_eq!(refreshes(&fake), ["alice-refresh"]);
    assert_eq!(usages(&fake).len(), asked);
    assert_eq!(live_access(&location), "fresh-access");
    assert_eq!(
        renewal(renewals(&app).await, "alice@example.com").as_deref(),
        Some("never")
    );
    // An account that had failed refreshes its usage once renewed.
    app.set_renewal(Pool::Claude, &bob, "never").await.unwrap();
    assert_eq!(
        app.refresh_usage(Pool::Claude, &bob).await.unwrap_err(),
        RENEWAL_OFF
    );
    app.renew_now(Pool::Claude, &bob).await.unwrap();
    assert_eq!(refreshes(&fake), ["alice-refresh", "bob-refresh"]);
    assert_eq!(usages(&fake).len(), asked + 1);
    let card = app
        .claude_load()
        .unwrap()
        .accounts
        .into_iter()
        .find(|a| a.id == bob)
        .unwrap();
    assert_eq!(card.usage_error, None);
    // A refused renewal is reported.
    *fake.refresh_error.lock().unwrap() = Some(SIGN_IN_EXPIRED.into());
    assert!(app.renew_now(Pool::Claude, &alice).await.is_err());
    assert!(app.renew_now(Pool::Claude, "missing").await.is_err());
}

#[test]
fn renewal_choices_read_and_default_as_shown() {
    for (text, choice) in [("never", Renewal::Never), ("always", Renewal::Always)] {
        assert_eq!(Renewal::parse(text), Ok(choice));
        assert_eq!(choice.as_str(), text);
        assert_eq!(serde_json::to_value(choice).unwrap(), json!(text));
        assert_eq!(Renewal::of(Some(choice)), choice);
    }
    assert_eq!(Renewal::of(None), Renewal::Always);
    assert!(Renewal::parse("Never").is_err());
    let legacy = json!({"id": "a", "label": "a", "document": {}, "account": {}, "addedAt": 1, "updatedAt": 1});
    let stored: Stored = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(stored.renewal, None);
    assert!(serde_json::to_value(&stored)
        .unwrap()
        .get("renewal")
        .is_none());
    // An earlier build's "once", shown as auto renewal off, is never.
    assert_eq!(Renewal::parse("once"), Ok(Renewal::Never));
    let mut once = legacy;
    once["renewal"] = "once".into();
    let stored: Stored = serde_json::from_value(once).unwrap();
    assert_eq!(stored.renewal, Some(Renewal::Never));
}

#[tokio::test]
async fn a_vault_without_the_tokens_moved_there_is_never_trusted_nor_written() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let (app, _, _) = vault_app(&dir, &vault);
    let (bob, account) = login("bob", LATER);
    app.claude_import_json(&export(&bob, &account))
        .await
        .unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    let held = vault.secrets();
    let writes = vault.writes();
    drop(app);
    // Reported missing although bob's tokens were moved there.
    vault.set(&vault.hidden, true);
    let (app, _, fake) = vault_app(&dir, &vault);
    let card = app.accounts(Pool::Claude).await.unwrap().remove(0);
    let error = card.usage_error.unwrap();
    assert!(error.starts_with(VAULT_LOCKED), "{error}");
    assert!(error.contains("reports the item missing"), "{error}");
    let refused = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
    assert!(refused.contains("reports the item missing"), "{refused}");
    assert!(usages(&fake).is_empty());
    // Saving, importing or asking writes nothing over it.
    let (carol, carol_account) = login("carol", LATER);
    app.claude_import_json(&export(&carol, &carol_account))
        .await
        .unwrap();
    assert!(store_text(&app).contains("carol-refresh"));
    let asked = vault.reads().len();
    assert!(app.claude_authorize().await.is_err());
    assert!(vault.reads()[asked..].contains(&Interactive));
    assert_eq!(vault.writes(), writes);
    assert_eq!(vault.secrets(), held);
    // Contents without bob's tokens are not the pool's either.
    vault.set(&vault.hidden, false);
    let text = vault.text.lock().unwrap().clone();
    *vault.text.lock().unwrap() = Some(r#"{"someone-else":{}}"#.into());
    let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
    assert!(
        error.contains("lacks the tokens of 1 account(s)"),
        "{error}"
    );
    assert_eq!(vault.writes(), writes);
    // Once it reads back as written, everything is there again.
    *vault.text.lock().unwrap() = text;
    app.claude_authorize().await.unwrap();
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(usages(&fake), ["bob-access"]);
    assert_no_tokens(&app, &["bob-refresh", "carol-refresh"]);
}

#[tokio::test]
async fn background_work_never_asks_for_keychain_access() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let (app, location, _) = vault_app(&dir, &vault);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    drop(app);
    let (app, _, _) = vault_app(&dir, &vault);
    let start = vault.reads().len();
    app.background_round(1, Pool::Claude).await;
    app.refresh_all(Pool::Claude).await.unwrap();
    app.accounts(Pool::Claude).await.unwrap();
    assert!(vault.reads()[start..].iter().all(|a| *a == Background));
    assert!(!vault.reads()[start..].is_empty());
    // A refusal is not asked again in the background, nor cached. (With
    // Claude Code's login present, its tokens would serve the account.)
    drop(app);
    fs::remove_file(location.credentials_file()).unwrap();
    vault.set(&vault.unreadable, true);
    let (app, _, _) = vault_app(&dir, &vault);
    let start = vault.reads().len();
    assert!(app
        .refresh_all(Pool::Claude)
        .await
        .unwrap_err()
        .contains(VAULT_LOCKED));
    assert!(vault.reads()[start..].iter().all(|a| *a == Background));
    assert!(vault.reads().len() - start >= 2);
    // The startup authorization asks, once.
    let start = vault.reads().len();
    app.claude_authorize_at_startup().await.unwrap();
    let asked = vault.reads()[start..]
        .iter()
        .filter(|a| **a == Interactive)
        .count();
    assert_eq!(asked, 1);
}

#[derive(Default)]
pub(super) struct Fake {
    usage: Mutex<Vec<(u16, Value)>>,
    usage_retry_after: Mutex<Option<u64>>,
    usage_clock_offset: Mutex<Option<i64>>,
    refreshed: Mutex<Vec<String>>,
    refresh_scopes: Mutex<Vec<String>>,
    profile_proxies: Mutex<Vec<String>>,
    usage_tokens: Mutex<Vec<String>>,
    usage_agents: Mutex<Vec<String>>,
    usage_proxies: Mutex<Vec<String>>,
    /// The OAuth API root of each usage, profile and prepaid request.
    api_roots: Mutex<Vec<String>>,
    usage_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    refresh_hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    refresh_error: Mutex<Option<String>>,
    /// The next token response, instead of the usual one.
    refresh_response: Mutex<Option<Value>>,
    prepaid: Mutex<Option<Value>>,
    prepaid_requests: Mutex<Vec<(String, String)>>,
    hellos: Mutex<Vec<(u16, Value)>>,
    hello_requests: Mutex<Vec<(String, String)>>,
    hello_urls: Mutex<Vec<String>>,
    hello_accounts: Mutex<Vec<String>>,
    /// Profile requests fail, as when the profile is out of reach.
    profile_fails: std::sync::atomic::AtomicBool,
    /// Claude Code's files: the profile of its access token is the
    /// account they name, as when Claude Code wrote both.
    claude_code: Mutex<Option<Location>>,
    /// The profile of an access token, before any other answer.
    profiles: Mutex<std::collections::BTreeMap<String, Value>>,
    /// Profile requests answer this late, in milliseconds.
    profile_delay_ms: std::sync::atomic::AtomicU64,
}
impl Network for Fake {
    fn usage<'a>(
        &'a self,
        api: &'a str,
        token: &'a str,
        agent: &'a str,
        proxy: &'a str,
    ) -> Response<'a, UsageResponse> {
        Box::pin(async move {
            self.api_roots.lock().unwrap().push(api.into());
            self.usage_tokens.lock().unwrap().push(token.into());
            self.usage_agents.lock().unwrap().push(agent.into());
            self.usage_proxies.lock().unwrap().push(proxy.into());
            let mut hooks = self.usage_hooks.lock().unwrap();
            if !hooks.is_empty() {
                hooks.remove(0)();
            }
            drop(hooks);
            let mut queued = self.usage.lock().unwrap();
            let (status, body) = if token.starts_with("expired") {
                (401, Value::Null)
            } else if queued.is_empty() {
                (
                    200,
                    json!({"five_hour":{"utilization":25,"resets_at":"2099-01-01T00:00:00Z"},"seven_day":{"utilization":"40.5","resets_at":null}}),
                )
            } else {
                queued.remove(0)
            };
            Ok((
                status,
                body,
                *self.usage_retry_after.lock().unwrap(),
                *self.usage_clock_offset.lock().unwrap(),
            ))
        })
    }
    fn refresh<'a>(&'a self, token: &'a str, scope: &'a str, _: &'a str) -> Response<'a, Value> {
        Box::pin(async move {
            self.refreshed.lock().unwrap().push(token.into());
            self.refresh_scopes.lock().unwrap().push(scope.into());
            let mut hooks = self.refresh_hooks.lock().unwrap();
            if !hooks.is_empty() {
                hooks.remove(0)();
            }
            drop(hooks);
            if let Some(error) = self.refresh_error.lock().unwrap().take() {
                return Err(error);
            }
            if let Some(response) = self.refresh_response.lock().unwrap().take() {
                return Ok(response);
            }
            Ok(
                json!({"access_token":"fresh-access","refresh_token":"fresh-refresh","expires_in":3600}),
            )
        })
    }
    fn profile<'a>(&'a self, api: &'a str, token: &'a str, proxy: &'a str) -> Response<'a, Value> {
        Box::pin(async move {
            self.api_roots.lock().unwrap().push(api.into());
            self.profile_proxies.lock().unwrap().push(proxy.into());
            let delay = self
                .profile_delay_ms
                .load(std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            if let Some(profile) = self.profiles.lock().unwrap().get(token) {
                return Ok(profile.clone());
            }
            let claude_code = self.claude_code.lock().unwrap().clone();
            let live = match claude_code {
                Some(location) => read_live(&location).await.ok().flatten(),
                None => None,
            };
            if let Some(live) = live {
                let holds = live.oauth().and_then(|o| text(&o["accessToken"]));
                let account = live.account.filter(|_| holds.as_deref() == Some(token));
                if let Some(account) = account {
                    return Ok(profile_of(&account));
                }
            }
            if self
                .profile_fails
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err("Claude profile request failed".into());
            }
            Ok(
                json!({"account":{"uuid":"acct-new","email":"new@example.com"},"organization":{"uuid":"org-new","name":"New","organization_type":"claude_max","rate_limit_tier":"default_claude_max_5x"}}),
            )
        })
    }
    fn prepaid<'a>(
        &'a self,
        api: &'a str,
        _: &'a str,
        org: &'a str,
        proxy: &'a str,
    ) -> Response<'a, Value> {
        Box::pin(async move {
            self.api_roots.lock().unwrap().push(api.into());
            self.prepaid_requests
                .lock()
                .unwrap()
                .push((org.into(), proxy.into()));
            self.prepaid
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| "HTTP 403".to_string())
        })
    }
    fn hello<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
        account: &'a str,
        proxy: &'a str,
    ) -> Response<'a, (u16, Value)> {
        Box::pin(async move {
            self.hello_urls.lock().unwrap().push(url.into());
            self.hello_accounts.lock().unwrap().push(account.into());
            self.hello_requests
                .lock()
                .unwrap()
                .push((token.into(), proxy.into()));
            let mut queued = self.hellos.lock().unwrap();
            Ok(if queued.is_empty() {
                (200, Value::Null)
            } else {
                queued.remove(0)
            })
        })
    }
}
fn login(user: &str, expires: i64) -> (Value, Value) {
    (
        json!({"accessToken": format!("{user}-access"), "refreshToken": format!("{user}-refresh"), "expiresAt": expires, "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"}),
        json!({"accountUuid": format!("acct-{user}"), "organizationUuid": format!("org-{user}"), "emailAddress": format!("{user}@example.com")}),
    )
}
#[tokio::test]
async fn startup_authorization_follows_the_live_account_without_refreshing() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "alice", LATER);

    app.claude_authorize_at_startup().await.unwrap();

    let store = app.claude_load().unwrap();
    let current = store
        .accounts
        .iter()
        .find(|a| Some(&a.id) == store.current_account_id.as_ref())
        .unwrap();
    assert_eq!(current.label, "alice@example.com");
    assert!(store.accounts.iter().all(|a| a.usage.is_none()));
    assert!(fake.usage.lock().unwrap().is_empty());
}

#[tokio::test]
async fn startup_authorization_skips_empty_pools_and_preserves_them_on_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = waiting_app(&dir);
    fs::create_dir_all(&location.config_dir).unwrap();
    fs::write(location.credentials_file(), "invalid credentials").unwrap();
    // An empty pool must not inspect even an existing credentials file.
    app.claude_authorize_at_startup().await.unwrap();

    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let before = fs::read(app.claude_store_path()).unwrap();
    fs::write(location.credentials_file(), "invalid credentials").unwrap();
    assert!(app.claude_authorize_at_startup().await.is_err());
    assert_eq!(fs::read(app.claude_store_path()).unwrap(), before);
}
pub(super) fn sign_in(location: &Location, user: &str, expires: i64) {
    let (oauth, account) = login(user, expires);
    fs::create_dir_all(&location.config_dir).unwrap();
    write_json(
        &location.credentials_file(),
        &json!({"claudeAiOauth": oauth, "mcpOAuth": {"kept": true}}),
    )
    .unwrap();
    write_account(location, &account).unwrap();
}
/// An isolated pool that contacts upstream right after an import.
async fn app(dir: &tempfile::TempDir) -> (Application, Location, Arc<Fake>) {
    let (app, location, fake) = waiting_app(dir);
    let preferences = Preferences {
        request_new_accounts: true,
        ..app.preferences().unwrap()
    };
    app.save_preferences(preferences).await.unwrap();
    (app, location, fake)
}
/// An isolated pool with the default: new accounts wait for a refresh.
fn waiting_app(dir: &tempfile::TempDir) -> (Application, Location, Arc<Fake>) {
    let item = crate::vault::Item::new(Arc::new(MemoryVault::default()));
    let app = Application::open_with_secrets(
        crate::Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        },
        Arc::new(crate::vault::ItemSecrets::new(item)),
    )
    .unwrap();
    let location = Location::files(dir.path().join("claude"));
    app.set_claude_location(location.clone());
    let fake = Arc::new(Fake::default());
    *fake.claude_code.lock().unwrap() = Some(location.clone());
    app.set_claude_network(fake.clone());
    (app, location, fake)
}
/// The profile Claude answers for an `oauthAccount`'s login.
fn profile_of(account: &Value) -> Value {
    json!({
        "account": {"uuid": account["accountUuid"], "email": account["emailAddress"]},
        "organization": {"uuid": account["organizationUuid"], "name": account["organizationName"]}
    })
}
fn live_access(location: &Location) -> String {
    let document: Value =
        serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
    document["claudeAiOauth"]["accessToken"]
        .as_str()
        .unwrap()
        .into()
}
const LATER: i64 = 4_000_000_000_000;

#[tokio::test]
async fn json_import_is_offline_deduplicated_and_only_switch_writes_live_login() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let credentials = fs::read(location.credentials_file()).unwrap();
    let config = fs::read(location.account_file()).unwrap();
    let (oauth, _) = login("bob", LATER);
    let document = json!({"claudeAiOauth": oauth, "mcpOAuth": {"foreign": true}});
    assert_eq!(
        app.claude_import_json(&document.to_string()).await.unwrap(),
        1
    );
    let imported = app.claude_load().unwrap();
    let bob = imported
        .accounts
        .iter()
        .find(|a| a.document == oauth)
        .unwrap();
    let bob_id = bob.id.clone();
    assert!(bob.first_request_pending);
    assert_ne!(
        imported.current_account_id.as_deref(),
        Some(bob_id.as_str())
    );
    assert!(fake.profile_proxies.lock().unwrap().is_empty());
    assert!(fake.refreshed.lock().unwrap().is_empty());
    // A bare token object and a BOM-prefixed export update the same entry.
    app.claude_import_json(&oauth.to_string()).await.unwrap();
    app.claude_import_json(&format!("\u{feff}{document}"))
        .await
        .unwrap();
    let repeated = app.claude_load().unwrap();
    assert_eq!(repeated.accounts.len(), 2);
    assert!(repeated.accounts.iter().any(|a| a.id == bob_id));
    assert_eq!(fs::read(location.credentials_file()).unwrap(), credentials);
    assert_eq!(fs::read(location.account_file()).unwrap(), config);
    app.switch_account(Pool::Claude, &bob_id).await.unwrap();
    assert_eq!(live_access(&location), "bob-access");
    let live = read_live(&location).await.unwrap().unwrap();
    assert_eq!(live.document["mcpOAuth"], json!({"kept": true}));
    assert!(live.account.is_none());
}

#[tokio::test]
async fn json_import_rejects_invalid_documents_without_changing_the_pool() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let saved = fs::read(app.claude_store_path()).unwrap();
    let live = fs::read(location.credentials_file()).unwrap();
    for invalid in [
        "not JSON",
        "null",
        "[]",
        "{}",
        "42",
        r#"{"claudeAiOauth":null}"#,
        r#"{"claudeAiOauth":{"accessToken":" "}}"#,
        r#"{"accessToken":123}"#,
        r#"{"accessToken":"test","refreshToken":42}"#,
        r#"{"accessToken":"test","expiresAt":"tomorrow"}"#,
        r#"{"accessToken":"test","scopes":[42]}"#,
        r#"{"tokens":{"access_token":"codex-token"}}"#,
    ] {
        assert_eq!(
            app.claude_import_json(invalid).await.unwrap_err(),
            "error.claude.credentials_json_invalid"
        );
        assert_eq!(fs::read(app.claude_store_path()).unwrap(), saved);
        assert_eq!(fs::read(location.credentials_file()).unwrap(), live);
    }
    // Over the limit, an import is refused unread.
    assert_eq!(
        app.import_json(Pool::Claude, &" ".repeat(16 * 1024 * 1024 + 1))
            .await
            .unwrap_err(),
        "Import exceeds 16 MiB"
    );
    // A file over the limit, or no file, is not read.
    let file = dir.path().join("big.json");
    fs::write(&file, " ".repeat(16 * 1024 * 1024 + 1)).unwrap();
    assert_eq!(
        app.import_file(Pool::Claude, &file).await.unwrap_err(),
        "File exceeds 16 MiB"
    );
    assert_eq!(
        app.import_file(Pool::Claude, dir.path()).await.unwrap_err(),
        "Not a file"
    );
    assert_eq!(fs::read(app.claude_store_path()).unwrap(), saved);
}

#[tokio::test]
async fn json_import_keeps_supplied_identity_or_resolves_it_when_requests_are_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    let (oauth, account) = login("bob", LATER);
    app.claude_import_json(&json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string())
        .await
        .unwrap();
    let store = app.claude_load().unwrap();
    assert_eq!(store.accounts[0].own.account, account);
    assert!(store.current_account_id.is_none());
    assert!(!location.credentials_file().exists());
    assert!(!location.account_file().exists());
    let (other, _) = login("carol", LATER);
    app.claude_import_json(&json!({"claudeAiOauth": other}).to_string())
        .await
        .unwrap();
    let store = app.claude_load().unwrap();
    assert_eq!(store.accounts.len(), 2);
    assert!(store
        .accounts
        .iter()
        .any(|a| a.own.account["emailAddress"] == "new@example.com"));
    assert!(!fake.profile_proxies.lock().unwrap().is_empty());
    assert!(!location.credentials_file().exists());
}

#[tokio::test]
async fn imports_switches_and_follows_the_claude_code_login() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = app(&dir).await;
    fs::create_dir_all(&location.config_dir).unwrap();
    fs::write(
        &location.global_config,
        "{\n  \"zeta\": 1,\n  \"oauthAccount\": null,\n  \"alpha\": [1, {\"a\": \"}\"}]\n}\n",
    )
    .unwrap();
    sign_in(&location, "alice", LATER);
    assert_eq!(app.claude_import_current().await.unwrap(), 1);
    assert_eq!(
        app.accounts(Pool::Claude).await.unwrap()[0].label,
        "alice@example.com"
    );
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts.iter().filter(|a| a.is_current).count(), 1);
    let bob = accounts.iter().find(|a| a.is_current).unwrap();
    assert_eq!(bob.email.as_deref(), Some("bob@example.com"));
    assert_eq!(bob.plan_type.as_deref(), Some("max 20x"));
    let usage = bob.usage.as_ref().unwrap();
    assert_eq!(usage.five_hour.as_ref().unwrap().used_percent, 25.0);
    assert_eq!(usage.one_week.as_ref().unwrap().used_percent, 40.5);
    assert!(!serde_json::to_string(&accounts).unwrap().contains("access"));

    let alice = accounts.iter().find(|a| !a.is_current).unwrap().id.clone();
    app.switch_account(Pool::Claude, &alice).await.unwrap();
    assert_eq!(live_access(&location), "alice-access");
    let document: Value =
        serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
    assert_eq!(document["mcpOAuth"]["kept"], true);
    let config = fs::read_to_string(&location.global_config).unwrap();
    assert!(config.starts_with("{\n  \"zeta\": 1,\n  \"oauthAccount\": {"));
    assert!(config.contains("\"alpha\": [1, {\"a\": \"}\"}]"));
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config["oauthAccount"]["accountUuid"], "acct-alice");
    // Bob is in the pool: only a login outside it is backed up.
    let backup = app.paths.data_dir.join("claude-before-switch.json");
    assert!(!backup.exists());
    let current = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(current.iter().find(|a| a.is_current).unwrap().id, alice);

    // A login made in Claude Code is followed, with the tokens it refreshed.
    let (mut oauth, account) = login("bob", LATER + 1);
    oauth["accessToken"] = "bob-newer".into();
    write_json(
        &location.credentials_file(),
        &json!({"claudeAiOauth": oauth}),
    )
    .unwrap();
    write_account(&location, &account).unwrap();
    let current = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(current.iter().find(|a| a.is_current).unwrap().id, bob.id);
    let stored = app.claude_load().unwrap();
    let bob_stored = stored.accounts.iter().find(|a| a.id == bob.id).unwrap();
    assert_eq!(bob_stored.document["accessToken"], "bob-newer");

    // A login outside the pool is backed up, and stays so across switches.
    sign_in(&location, "carol", LATER);
    app.switch_account(Pool::Claude, &alice).await.unwrap();
    let read_backup = || serde_json::from_slice::<Value>(&fs::read(&backup).unwrap()).unwrap();
    let backed_up = read_backup();
    assert_eq!(
        backed_up["credentials"]["claudeAiOauth"]["accessToken"],
        "carol-access"
    );
    app.switch_account(Pool::Claude, &bob.id).await.unwrap();
    assert_eq!(read_backup(), backed_up);
}

#[tokio::test]
async fn switching_hands_the_login_and_identity_to_the_switch_effects() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    let alice = app
        .accounts(Pool::Claude)
        .await
        .unwrap()
        .into_iter()
        .find(|a| !a.is_current)
        .unwrap()
        .id;
    let seen = Arc::new(std::sync::Mutex::new(None));
    let observed = seen.clone();
    app.on_switch_effects(Arc::new(move |pool, _, credential| {
        let observed = observed.clone();
        Box::pin(async move {
            *observed.lock().unwrap() = Some((pool, credential));
            Ok(SwitchExecution::default())
        })
    }));
    app.switch_account(Pool::Claude, &alice).await.unwrap();
    let (pool, credential) = seen.lock().unwrap().take().unwrap();
    assert_eq!(pool, Pool::Claude);
    let credential = credential.unwrap().unwrap();
    assert_eq!(credential.access, "alice-access");
    assert_eq!(credential.account_id.as_deref(), Some("acct-alice"));
}

fn refreshes(fake: &Fake) -> Vec<String> {
    fake.refreshed.lock().unwrap().clone()
}
fn usages(fake: &Fake) -> Vec<String> {
    fake.usage_tokens.lock().unwrap().clone()
}
/// Claude answers the next usage request with 401 and refuses the next
/// refresh token.
fn reject_next(fake: &Fake) {
    fake.usage.lock().unwrap().push((401, Value::Null));
    *fake.refresh_error.lock().unwrap() = Some(SIGN_IN_EXPIRED.into());
}

#[tokio::test]
async fn an_earlier_expiring_live_login_is_not_followed_but_imported() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", LATER + 1);
    app.claude_import_current().await.unwrap();
    let stored = || app.claude_load().unwrap().accounts[0].clone();
    let pool = stored().document;
    // Claude Code's login expires earlier than the pool's copy.
    let (mut oauth, account) = login("alice", LATER);
    oauth["refreshToken"] = "alice-live".into();
    write_json(
        &location.credentials_file(),
        &json!({"claudeAiOauth": oauth}),
    )
    .unwrap();
    write_account(&location, &account).unwrap();
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(stored().document, pool);
    assert_eq!(
        app.claude_load().unwrap().current_account_id,
        Some(stored().id)
    );
    // Importing the current login takes it, whatever its expiry.
    app.claude_import_current().await.unwrap();
    assert_eq!(stored().document, oauth);
    assert!(refreshes(&fake).is_empty());
    assert!(usages(&fake).is_empty());
}

#[tokio::test]
async fn claude_code_renewing_or_signing_in_is_followed() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    let account = |user: &str| {
        app.claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.label == format!("{user}@example.com"))
            .unwrap()
    };
    app.switch_account(Pool::Claude, &account("alice").id)
        .await
        .unwrap();
    // Claude Code renews alice: the pool takes the renewal.
    external_update(location.clone(), "alice", "alice-renewed")();
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(account("alice").document["accessToken"], "alice-renewed");
    // A sign-in to bob there becomes bob's, and current.
    external_update(location.clone(), "bob", "bob-login")();
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(account("bob").document["accessToken"], "bob-login");
    assert_eq!(
        app.claude_load().unwrap().current_account_id,
        Some(account("bob").id)
    );
    assert!(refreshes(&fake).is_empty());
    assert!(usages(&fake).is_empty());
}

#[tokio::test]
async fn a_new_login_read_beside_the_account_it_replaced_never_becomes_its() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let stored = || app.claude_load().unwrap().accounts[0].clone();
    let alice = stored();
    // Claude Code signs in to carol: her tokens are written, alice's
    // oauthAccount not yet replaced.
    let torn = || {
        let (mut oauth, _) = login("carol", LATER + 1);
        oauth["accessToken"] = "carol-access".into();
        write_json(
            &location.credentials_file(),
            &json!({"claudeAiOauth": oauth}),
        )
        .unwrap();
    };
    torn();
    let (_, carol) = login("carol", LATER);
    fake.profiles
        .lock()
        .unwrap()
        .insert("carol-access".into(), profile_of(&carol));
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(stored().document, alice.document);
    assert_eq!(app.claude_load().unwrap().current_account_id, None);
    // Claude Code's oauthAccount is left for Claude Code to write.
    let config: Value =
        serde_json::from_slice(&fs::read(location.account_file()).unwrap()).unwrap();
    assert_eq!(config["oauthAccount"]["accountUuid"], "acct-alice");
}

#[tokio::test]
async fn a_token_its_profile_named_is_not_asked_about_again() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let earlier = app.claude_load().unwrap();
    // Claude Code renews alice's login: its profile confirms it.
    external_update(location.clone(), "alice", "alice-renewed")();
    app.accounts(Pool::Claude).await.unwrap();
    let access =
        |app: &Application| app.claude_load().unwrap().accounts[0].document["accessToken"].clone();
    assert_eq!(access(&app), "alice-renewed");
    let asked = fake.profile_proxies.lock().unwrap().len();
    // Read again before the store holds it, as a renewal under Claude
    // Code's lock rereads it, the token is not asked about again: its
    // profile, refused or out of reach by now, would hold it unconfirmed.
    app.claude_save(&earlier).unwrap();
    *fake.claude_code.lock().unwrap() = None;
    fake.profile_fails
        .store(true, std::sync::atomic::Ordering::Relaxed);
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(fake.profile_proxies.lock().unwrap().len(), asked);
    assert_eq!(access(&app), "alice-renewed");
}

#[tokio::test]
async fn a_login_not_yet_confirmed_is_as_one_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    // Alice's copy has expired; Claude Code holds a later login naming
    // her, which may be carol's.
    let mut store = app.claude_load().unwrap();
    let alice = store
        .accounts
        .iter()
        .position(|a| a.label == "alice@example.com")
        .unwrap();
    store.accounts[alice].document["expiresAt"] = 1.into();
    app.claude_save(&store).unwrap();
    let (alice, bob) = (
        store.accounts[alice].clone(),
        store.accounts[1 - alice].clone(),
    );
    let (mut oauth, _) = login("carol", LATER + 1);
    oauth["accessToken"] = "carol-access".into();
    write_json(
        &location.credentials_file(),
        &json!({"claudeAiOauth": oauth}),
    )
    .unwrap();
    *fake.claude_code.lock().unwrap() = None;
    fake.profile_fails
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let renewed = refreshes(&fake).len();
    // The cards keep the selection, and alice's copy.
    app.accounts(Pool::Claude).await.unwrap();
    let store = app.claude_load().unwrap();
    let stored = store.accounts.iter().find(|a| a.id == alice.id).unwrap();
    assert_eq!(stored.document, alice.document);
    assert_eq!(store.current_account_id.as_ref(), Some(&alice.id));
    // Asked once within a minute, however often the cards are read.
    let asked = fake.profile_proxies.lock().unwrap().len();
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(fake.profile_proxies.lock().unwrap().len(), asked);
    // The account Claude Code may be using is not renewed meanwhile.
    let error = app
        .refresh_usage(Pool::Claude, &alice.id)
        .await
        .unwrap_err();
    assert!(error.contains(LIVE_UNREADABLE), "{error}");
    assert_eq!(refreshes(&fake).len(), renewed);
    // A switch is made, the login it leaves kept in the backup.
    app.switch_account(Pool::Claude, &bob.id).await.unwrap();
    assert_eq!(live_access(&location), "bob-access");
    let backup: Value = serde_json::from_slice(
        &fs::read(app.paths.data_dir.join("claude-before-switch.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        backup["credentials"]["claudeAiOauth"]["accessToken"],
        "carol-access"
    );
}

#[tokio::test]
async fn the_cards_wait_for_a_login_to_be_confirmed_only_briefly() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let alice = app.claude_load().unwrap().accounts[0].clone();
    external_update(location.clone(), "alice", "alice-renewed")();
    fake.profile_delay_ms
        .store(60_000, std::sync::atomic::Ordering::Relaxed);
    let started = std::time::Instant::now();
    app.accounts(Pool::Claude).await.unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    // Not confirmed in time: not taken yet.
    let stored = app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(stored.document, alice.document);
}

#[tokio::test]
async fn a_later_login_written_during_a_renewal_is_followed_afterwards() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    let (oauth, account) = login("alice", 1);
    app.claude_import_json(&json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string())
        .await
        .unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    // Unreadable when the refresh starts, an older login once it renews.
    fs::create_dir_all(&location.config_dir).unwrap();
    fs::write(location.credentials_file(), "not json").unwrap();
    fake.refresh_hooks
        .lock()
        .unwrap()
        .push(external_update(location.clone(), "alice", "stale"));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(refreshes(&fake), ["alice-refresh"]);
    assert_eq!(usages(&fake), ["fresh-access"]);
    let stored = || app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(stored().document["accessToken"], "fresh-access");
    // Claude Code's later login is followed once read.
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(stored().document["accessToken"], "stale");
}

#[tokio::test]
async fn a_refresh_keeps_a_generation_adopted_while_it_ran() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _, fake) = waiting_app(&dir);
    let (oauth, account) = login("bob", LATER);
    app.claude_import_json(&json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string())
        .await
        .unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    let (mut other, _) = login("bob", LATER + 1);
    other["accessToken"] = "bob-other".into();
    other["refreshToken"] = "bob-other-refresh".into();
    let (path, adopted) = (app.claude_store_path(), other.clone());
    fake.usage_hooks.lock().unwrap().push(Box::new(move || {
        let mut store: Store = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(adopt(&mut store.accounts[0], &adopted));
        write_json(&path, &store).unwrap();
    }));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    let stored = app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(stored.document, other);
    assert!(stored.usage.is_some());
    assert_eq!(usages(&fake), ["bob-access"]);
}

/// The Keychain write can succeed before the fallback file refuses it.
/// Exercise the real switch and file writer with an in-memory item only.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_failed_mirror_write_restores_the_keychain_login() {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut location, _) = waiting_app(&dir);
    let (bob_oauth, bob_account) = login("bob", LATER);
    app.claude_import_json(
        &json!({"claudeAiOauth": bob_oauth, "oauthAccount": bob_account}).to_string(),
    )
    .await
    .unwrap();
    let bob = app.claude_load().unwrap().accounts[0].id.clone();
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let before = app.claude_load().unwrap().current_account_id;
    let account_file = fs::read(location.account_file()).unwrap();
    let previous: Value =
        serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
    let item = keychain::testing::Item::new(previous.clone());
    location.keychain = Some(item.0.clone());
    app.set_claude_location(location.clone());
    // A directory cannot be replaced by the atomic JSON file writer,
    // even when the tests run with elevated permissions.
    fs::remove_file(location.credentials_file()).unwrap();
    fs::create_dir(location.credentials_file()).unwrap();

    assert!(app.switch_account(Pool::Claude, &bob).await.is_err());
    let mut target = previous.clone();
    target["claudeAiOauth"] = bob_oauth;
    // Both writes really happened: the target, then the restored login.
    assert_eq!(item.writes(), [target, previous.clone()]);
    assert_eq!(
        keychain::read(&item.0, KeychainAccess::Background)
            .await
            .unwrap(),
        Some(previous)
    );
    assert_eq!(fs::read(location.account_file()).unwrap(), account_file);
    assert_eq!(app.claude_load().unwrap().current_account_id, before);
}

#[tokio::test]
async fn a_failed_switch_leaves_no_login_where_there_was_none() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    fs::remove_file(location.credentials_file()).unwrap();
    // The identity cannot be written after the credentials are.
    fs::remove_file(location.account_file()).unwrap();
    fs::create_dir(location.account_file()).unwrap();
    assert!(app.switch_account(Pool::Claude, &id).await.is_err());
    assert!(!location.credentials_file().exists());
}

#[tokio::test]
async fn a_live_login_renews_and_syncs_without_changing_other_fields() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", 1);
    let before: Value =
        serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
    let config_before = fs::read(location.account_file()).unwrap();
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        app.refresh_usage(Pool::Claude, &id),
        app.refresh_usage(Pool::Claude, &id)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
    assert_eq!(
        app.claude_load().unwrap().accounts[0].document["accessToken"],
        "fresh-access"
    );
    // The active login and pool share the refreshed generation.
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(
        app.claude_load().unwrap().accounts[0].document["refreshToken"],
        "fresh-refresh"
    );
    fake.usage.lock().unwrap().push((401, Value::Null));
    *fake.refresh_response.lock().unwrap() = Some(
        json!({"access_token":"fresher-access","refresh_token":"fresher-refresh","expires_in":3600}),
    );
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(
        *fake.refreshed.lock().unwrap(),
        vec!["alice-refresh", "fresh-refresh"]
    );
    let after: Value =
        serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
    assert_eq!(
        after["claudeAiOauth"],
        app.claude_load().unwrap().accounts[0].document
    );
    assert_eq!(after["mcpOAuth"], before["mcpOAuth"]);
    assert_eq!(fs::read(location.account_file()).unwrap(), config_before);
}

/// The live login, signed in with an expired token, imported and released.
async fn expired_live_login(dir: &tempfile::TempDir) -> (Application, Location, Arc<Fake>, String) {
    let (app, location, fake) = waiting_app(dir);
    sign_in(&location, "alice", 1);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    (app, location, fake, id)
}
fn claude_code_locks(location: &Location) -> [PathBuf; 2] {
    refresh_lock::lock_paths(&location.storage_dir)
}
fn hold_claude_code_locks(location: &Location) -> [PathBuf; 2] {
    let locks = claude_code_locks(location);
    for lock in &locks {
        fs::create_dir(lock).unwrap();
    }
    locks
}

#[tokio::test]
async fn renewal_waits_while_claude_code_renews_and_never_spends_its_token() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake, id) = expired_live_login(&dir).await;
    let live = fs::read(location.credentials_file()).unwrap();
    // Claude Code holds its lock throughout: nothing is renewed or written.
    let locks = hold_claude_code_locks(&location);
    let error = app.refresh_usage(Pool::Claude, &id).await.unwrap_err();
    assert!(error.contains(CLAUDE_CODE_RENEWING), "{error}");
    assert!(refreshes(&fake).is_empty());
    assert!(usages(&fake).is_empty());
    assert_eq!(fs::read(location.credentials_file()).unwrap(), live);
    // Another holder's lock is never removed.
    assert!(locks.iter().all(|lock| lock.is_dir()));
    // Only the legacy lock, as an older Claude Code takes it, also counts.
    fs::remove_dir(&locks[0]).unwrap();
    assert!(app.refresh_usage(Pool::Claude, &id).await.is_err());
    assert!(refreshes(&fake).is_empty());
    assert!(!locks[0].exists() && locks[1].is_dir());

    // Claude Code finishes: it renewed the same login and released.
    fs::remove_dir(&locks[1]).unwrap();
    hold_claude_code_locks(&location);
    let finish = {
        let location = location.clone();
        let locks = locks.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            external_update(location, "alice", "code")();
            for lock in locks.iter().rev() {
                fs::remove_dir(lock).unwrap();
            }
        })
    };
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    finish.join().unwrap();
    // Its renewal is adopted; the token it spent is not sent again.
    assert!(refreshes(&fake).is_empty());
    assert_eq!(usages(&fake), ["code"]);
    let stored = app.claude_load().unwrap();
    assert_eq!(stored.accounts[0].document["refreshToken"], "code-refresh");
    assert_eq!(live_access(&location), "code");
    assert!(locks.iter().all(|lock| !lock.exists()));
}

#[tokio::test]
async fn renewing_now_takes_a_renewal_claude_code_made_meanwhile_instead() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake, id) = expired_live_login(&dir).await;
    // Claude Code renews while Copool waits for its lock.
    let locks = hold_claude_code_locks(&location);
    let finish = {
        let location = location.clone();
        let locks = locks.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            external_update(location, "alice", "code")();
            for lock in locks.iter().rev() {
                fs::remove_dir(lock).unwrap();
            }
        })
    };
    app.renew_now(Pool::Claude, &id).await.unwrap();
    finish.join().unwrap();
    // Its renewal is kept and serves as one, not renewed again.
    assert!(refreshes(&fake).is_empty());
    let stored = app.claude_load().unwrap();
    assert_eq!(stored.accounts[0].document["refreshToken"], "code-refresh");
    assert_eq!(live_access(&location), "code");
}

#[tokio::test]
async fn renewal_reaches_claude_codes_login_under_its_lock_before_usage_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake, id) = expired_live_login(&dir).await;
    // Members Copool does not keep: Claude Code's client and MCP logins.
    let mut document: Value =
        serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
    document["claudeAiOauth"]["clientId"] = "code-client".into();
    write_json(&location.credentials_file(), &document).unwrap();
    let locks = claude_code_locks(&location);
    let observed = Arc::new(Mutex::new(Vec::new()));
    fake.refresh_hooks.lock().unwrap().push(Box::new({
        let (locks, observed) = (locks.clone(), observed.clone());
        move || {
            let held = locks.iter().all(|lock| lock.is_dir());
            observed
                .lock()
                .unwrap()
                .push(format!("refresh locked={held}"));
        }
    }));
    fake.usage_hooks.lock().unwrap().push(Box::new({
        let (locks, location, observed) = (locks.clone(), location.clone(), observed.clone());
        move || {
            let held = locks.iter().any(|lock| lock.exists());
            observed.lock().unwrap().push(format!(
                "usage locked={held} live={}",
                live_access(&location)
            ));
        }
    }));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(
        *observed.lock().unwrap(),
        [
            "refresh locked=true",
            "usage locked=false live=fresh-access"
        ]
    );
    let live: Value =
        serde_json::from_slice(&fs::read(location.credentials_file()).unwrap()).unwrap();
    assert_eq!(live["claudeAiOauth"]["refreshToken"], "fresh-refresh");
    assert_eq!(live["claudeAiOauth"]["clientId"], "code-client");
    assert_eq!(live["mcpOAuth"], json!({"kept": true}));
    let stored = app.claude_load().unwrap();
    // The pool and Claude Code hold one generation.
    assert!(same_oauth_generation(
        &stored.accounts[0].document,
        &live["claudeAiOauth"]
    ));
    assert_eq!(refreshes(&fake), ["alice-refresh"]);
}

#[cfg(unix)]
#[tokio::test]
async fn renewal_takes_over_from_a_claude_code_that_died_holding_its_lock() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake, id) = expired_live_login(&dir).await;
    // Exited mid-renewal: its locks are far from stale, but its owner
    // record proves it gone.
    let locks = refresh_lock::hold_as_dead_claude_code(&location.storage_dir);
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(refreshes(&fake), ["alice-refresh"]);
    assert_eq!(live_access(&location), "fresh-access");
    assert!(locks.iter().all(|lock| !lock.exists()));
    assert!(!location.storage_dir.join(refresh_lock::OWNER_FILE).exists());
}

#[tokio::test]
async fn switching_waits_until_claude_code_has_renewed_the_login_being_left() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    let alice = app
        .claude_load()
        .unwrap()
        .accounts
        .iter()
        .find(|a| a.label == "alice@example.com")
        .unwrap()
        .id
        .clone();
    let locks = hold_claude_code_locks(&location);
    // The cards are read while it waits.
    let (switched, read) = tokio::join!(app.switch_account(Pool::Claude, &alice), async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        app.claude.logins.store.try_lock().is_ok()
    });
    let error = switched.unwrap_err();
    assert_eq!(error, CLAUDE_CODE_RENEWING_SWITCH);
    assert!(read);
    assert_eq!(live_access(&location), "bob-access");
    for lock in locks.iter().rev() {
        fs::remove_dir(lock).unwrap();
    }
    app.switch_account(Pool::Claude, &alice).await.unwrap();
    assert_eq!(live_access(&location), "alice-access");
    assert!(locks.iter().all(|lock| !lock.exists()));
}

#[tokio::test]
async fn pool_logins_renew_without_creating_a_claude_code_directory() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    let (oauth, account) = login("bob", 1);
    app.claude_import_json(&json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string())
        .await
        .unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(refreshes(&fake), ["bob-refresh"]);
    assert!(!location.storage_dir.exists());
    let stored = app.claude_load().unwrap();
    assert_eq!(stored.accounts[0].document["refreshToken"], "fresh-refresh");
}

/// Where Claude Code would keep its login in the Keychain but keeps
/// none, the pool's renewals create no directory for its lock either.
/// Not on macOS, where the Keychain read would reach the user's.
#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn pool_logins_renew_without_creating_a_directory_for_a_keychain_never_used() {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut location, fake) = waiting_app(&dir);
    location.keychain = Some("Copool test credentials".into());
    app.set_claude_location(location.clone());
    *fake.claude_code.lock().unwrap() = Some(location.clone());
    let (oauth, account) = login("bob", 1);
    app.claude_import_json(&json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string())
        .await
        .unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(refreshes(&fake), ["bob-refresh"]);
    assert!(!location.storage_dir.exists());
}

#[tokio::test]
async fn a_pool_login_renews_after_waiting_for_claude_code_to_renew_another() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let (oauth, account) = login("bob", 1);
    app.claude_import_json(&json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string())
        .await
        .unwrap();
    let bob = app
        .claude_load()
        .unwrap()
        .accounts
        .iter()
        .find(|a| a.document["refreshToken"] == "bob-refresh")
        .unwrap()
        .id
        .clone();
    app.release_new_accounts(Pool::Claude, Some(&bob))
        .await
        .unwrap();
    // Claude Code renews its own login, alice's, while bob's waits.
    let locks = hold_claude_code_locks(&location);
    let finish = {
        let location = location.clone();
        let locks = locks.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            external_update(location, "alice", "code")();
            for lock in locks.iter().rev() {
                fs::remove_dir(lock).unwrap();
            }
        })
    };
    app.refresh_usage(Pool::Claude, &bob).await.unwrap();
    finish.join().unwrap();
    assert_eq!(refreshes(&fake), ["bob-refresh"]);
    assert_eq!(live_access(&location), "code");
    let stored = app.claude_load().unwrap();
    let saved = stored.accounts.iter().find(|a| a.id == bob).unwrap();
    assert_eq!(saved.document["refreshToken"], "fresh-refresh");
    assert!(saved.usage_error.is_none());
}

#[tokio::test]
async fn live_rotation_is_preserved_even_when_usage_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", 1);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    fake.usage.lock().unwrap().push((500, Value::Null));
    assert!(app
        .refresh_usage(Pool::Claude, &id)
        .await
        .unwrap_err()
        .contains("HTTP 500"));
    assert_eq!(live_access(&location), "fresh-access");
    assert_eq!(
        app.claude_load().unwrap().accounts[0].document["refreshToken"],
        "fresh-refresh"
    );
}

#[tokio::test]
async fn an_inactive_login_renews_once_without_writing_live_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", 1);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    sign_in(&location, "bob", LATER);
    let before = fs::read(location.credentials_file()).unwrap();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        app.refresh_usage(Pool::Claude, &id),
        app.refresh_usage(Pool::Claude, &id)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
    assert_eq!(fs::read(location.credentials_file()).unwrap(), before);
    assert_eq!(
        app.claude_load().unwrap().accounts[0].document["refreshToken"],
        "fresh-refresh"
    );
    fake.usage
        .lock()
        .unwrap()
        .extend([(401, Value::Null), (401, Value::Null)]);
    assert_eq!(
        app.refresh_usage(Pool::Claude, &id).await.unwrap_err(),
        SIGN_IN_EXPIRED
    );
}

#[tokio::test]
async fn a_card_refresh_follows_rotated_tokens_without_renewing_them() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let (mut oauth, _) = login("alice", LATER + 1);
    oauth["refreshToken"] = "rotated-refresh".into();
    write_json(
        &location.credentials_file(),
        &json!({"claudeAiOauth": oauth}),
    )
    .unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert_eq!(app.claude_load().unwrap().accounts[0].document, oauth);
    assert!(fake.refreshed.lock().unwrap().is_empty());
}

fn external_update(location: Location, user: &str, access: &str) -> Box<dyn FnOnce() + Send> {
    let user = user.to_owned();
    let access = access.to_owned();
    Box::new(move || {
        // A renewal lasts longer than the logins the tests sign in with.
        let (mut oauth, account) = login(&user, LATER + 1);
        oauth["accessToken"] = access.clone().into();
        oauth["refreshToken"] = format!("{access}-refresh").into();
        write_json(
            &location.credentials_file(),
            &json!({"claudeAiOauth": oauth}),
        )
        .unwrap();
        write_account(&location, &account).unwrap();
    })
}

#[tokio::test]
async fn external_rotation_wins_over_refresh_success_or_failure() {
    for failed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = waiting_app(&dir);
        sign_in(&location, "alice", 1);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.release_new_accounts(Pool::Claude, Some(&id))
            .await
            .unwrap();
        fake.refresh_hooks.lock().unwrap().push(external_update(
            location.clone(),
            "alice",
            "external",
        ));
        if failed {
            *fake.refresh_error.lock().unwrap() = Some(SIGN_IN_EXPIRED.into());
        }
        let refreshed = app.refresh_usage(Pool::Claude, &id).await;
        assert_eq!(refreshed.is_err(), failed);
        // Claude Code's later login is followed, whatever became of the
        // renewal, and its token serves the next refresh.
        app.accounts(Pool::Claude).await.unwrap();
        app.refresh_usage(Pool::Claude, &id).await.unwrap();
        let stored = app.claude_load().unwrap();
        assert_eq!(stored.accounts[0].document["accessToken"], "external");
        assert_eq!(
            stored.accounts[0].document["refreshToken"],
            "external-refresh"
        );
        assert!(stored.accounts[0].usage_error.is_none());
        assert_eq!(*fake.refreshed.lock().unwrap(), vec!["alice-refresh"]);
        assert_eq!(
            fake.usage_tokens.lock().unwrap().last().unwrap(),
            "external"
        );
        assert_eq!(live_access(&location), "external");
    }
}

#[tokio::test]
async fn nameless_rotation_during_a_401_uses_the_verified_token_without_renewal() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "new", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    let changed = location.clone();
    fake.usage_hooks.lock().unwrap().push(Box::new(move || {
        sign_in_unnamed(&changed, "rotated");
    }));
    fake.usage.lock().unwrap().push((401, Value::Null));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    assert!(fake.refreshed.lock().unwrap().is_empty());
    assert_eq!(usages(&fake), ["new-access", "rotated-access"]);
    assert_eq!(
        app.claude_load().unwrap().accounts[0].document["refreshToken"],
        "rotated-refresh"
    );
}

#[tokio::test]
async fn external_account_switch_does_not_replace_the_refreshed_account() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", 1);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.release_new_accounts(Pool::Claude, Some(&id))
        .await
        .unwrap();
    fake.refresh_hooks
        .lock()
        .unwrap()
        .push(external_update(location.clone(), "bob", "bob-new"));
    app.refresh_usage(Pool::Claude, &id).await.unwrap();
    let stored = app.claude_load().unwrap();
    assert_eq!(stored.accounts[0].document["accessToken"], "fresh-access");
    assert_eq!(stored.accounts[0].own.account["accountUuid"], "acct-alice");
    assert_eq!(live_access(&location), "bob-new");
}

#[test]
fn generation_comparison_includes_expiry_but_ignores_metadata() {
    let (oauth, _) = login("alice", LATER);
    let mut changed = oauth.clone();
    changed["subscriptionType"] = "pro".into();
    assert!(same_oauth_generation(&oauth, &changed));
    changed["expiresAt"] = (LATER + 1).into();
    assert!(!same_oauth_generation(&oauth, &changed));
}

#[tokio::test]
async fn a_browser_sign_in_is_stored_without_changing_claude_code() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    let count = app
            .claude_import_login(json!({"access_token":"new-access","refresh_token":"new-refresh","expires_in":28800,"scope":"user:inference user:profile"}))
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert_eq!(live_access(&location), "alice-access");
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].label, "new@example.com");
    assert!(!accounts[0].is_current);
    assert_eq!(accounts[0].plan_type.as_deref(), Some("max 5x"));
    // The only stored account is not Claude Code's, so Smart Switch takes it.
    assert_eq!(
        app.smart_switch(Pool::Claude).await.unwrap(),
        SmartSwitchOutcome::Switched {
            label: "new@example.com".into(),
            execution: SwitchExecution::default(),
        }
    );
    assert_eq!(live_access(&location), "new-access");
}

#[tokio::test]
async fn a_hi_is_said_as_claude_code_in_a_terminal_session() {
    use axum::{extract::Request, routing::any, Json, Router};
    let seen = Arc::new(Mutex::new(None));
    let recorder = seen.clone();
    let router = Router::new().fallback(any(move |request: Request| {
        let recorder = recorder.clone();
        async move {
            let headers = request.headers().clone();
            let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            *recorder.lock().unwrap() = Some((headers, body));
            Json(json!({"type": "message"}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (status, _) = Http.hello(&url, "token", "acct-alice", "").await.unwrap();
    assert_eq!(status, 200);
    let (headers, body) = seen.lock().unwrap().take().unwrap();
    let header = |name: &str| headers.get(name).unwrap().to_str().unwrap().to_owned();
    assert_eq!(header("user-agent"), api::INTERACTIVE_USER_AGENT);
    assert_eq!(header("x-app"), "cli");
    assert_eq!(header("authorization"), "Bearer token");
    let user: Value = serde_json::from_str(body["metadata"]["user_id"].as_str().unwrap()).unwrap();
    assert_eq!(user["account_uuid"], "acct-alice");
    assert_eq!(user["session_id"], header("x-claude-code-session-id"));
    assert_eq!(user["device_id"].as_str().unwrap().len(), 64);
    assert_eq!(
        (body["model"].as_str(), body["max_tokens"].as_i64()),
        (Some(HELLO_MODEL), Some(1))
    );
    // Sonnet 5.5 refuses thinking turned off.
    assert!(body["thinking"].is_null());
}

#[tokio::test]
async fn oauth_api_requests_follow_the_custom_oauth_url_as_claude_code_reads_it() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts.remove(0).id;
    let mut store = app.claude_load().unwrap();
    store.accounts[0].own.account["organizationUuid"] = "org-alice".into();
    app.claude_save(&store).unwrap();
    *fake.prepaid.lock().unwrap() = Some(json!({"amount": 0}));
    fake.usage.lock().unwrap().extend((0..5).map(|_| {
        (
            200,
            json!({"five_hour": {"utilization": 1}, "extra_usage": {}}),
        )
    }));
    let roots = |env: Option<&str>, settings: Option<Value>| {
        app.set_claude_location(Location {
            custom_oauth_env: env.map(str::to_owned),
            ..location.clone()
        });
        let text = settings.map(|env| json!({ "env": env }).to_string());
        put_settings(&location, text.as_deref()).unwrap();
        let (app, fake, id) = (&app, &fake, &id);
        async move {
            fake.api_roots.lock().unwrap().clear();
            app.refresh_usage(Pool::Claude, id).await.unwrap();
            let roots = fake.api_roots.lock().unwrap().clone();
            assert!(roots.len() >= 2, "usage and prepaid: {roots:?}");
            roots.into_iter().collect::<std::collections::BTreeSet<_>>()
        }
    };
    let only = |root: &str| std::collections::BTreeSet::from([root.to_owned()]);
    assert_eq!(roots(None, None).await, only("https://api.anthropic.com"));
    // The settings' value wins over the environment, without one slash.
    assert_eq!(
        roots(
            Some("http://env.invalid"),
            Some(json!({CUSTOM_OAUTH: "http://127.0.0.1:8787/anthropic/"}))
        )
        .await,
        only("http://127.0.0.1:8787/anthropic")
    );
    // An empty value in the settings unsets the environment's.
    assert_eq!(
        roots(Some("http://env.invalid"), Some(json!({CUSTOM_OAUTH: ""}))).await,
        only("https://api.anthropic.com")
    );
    assert_eq!(
        roots(Some("http://env.invalid/"), Some(json!({}))).await,
        only("http://env.invalid")
    );
}

#[tokio::test]
async fn saying_hi_goes_where_claude_code_sends_the_login() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts.remove(0).id;
    let settings =
        |env: Value| put_settings(&location, Some(&json!({ "env": env }).to_string())).unwrap();
    async fn said(app: &Application, fake: &Fake, id: &str) -> String {
        app.say_hi(Pool::Claude, id).await.unwrap();
        fake.hello_urls.lock().unwrap().pop().unwrap()
    }
    assert_eq!(
        said(&app, &fake, &id).await,
        "https://api.anthropic.com/v1/messages"
    );
    // A relay forwarding the login, with blank credentials beside it.
    settings(json!({BASE_URL: "http://127.0.0.1:8787/anthropic/",
            "ANTHROPIC_AUTH_TOKEN": "", "ANTHROPIC_API_KEY": ""}));
    assert_eq!(
        said(&app, &fake, &id).await,
        "http://127.0.0.1:8787/anthropic/v1/messages"
    );
    // A base URL beside a credential is that credential's gateway.
    settings(json!({BASE_URL: "https://gateway.invalid", "ANTHROPIC_AUTH_TOKEN": "sk"}));
    assert_eq!(
        said(&app, &fake, &id).await,
        "https://api.anthropic.com/v1/messages"
    );
    // While a provider is applied, the official settings recorded apply.
    app.claude_record_official(&Official {
        settings: Some(json!({"env": {BASE_URL: "http://relay.invalid"}}).to_string()),
        applied: None,
    })
    .unwrap();
    let mut store = app.claude_load().unwrap();
    store.settings_provider = Some("provider".into());
    app.claude_save(&store).unwrap();
    assert_eq!(
        said(&app, &fake, &id).await,
        "http://relay.invalid/v1/messages"
    );
}

#[tokio::test]
async fn saying_hi_goes_through_the_account_proxy_and_then_shows_the_usage() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts.remove(0).id;
    app.save_account_proxy(Pool::Claude, &id, "socks5://127.0.0.1:7891")
        .await
        .unwrap();
    let usages = fake.usage_tokens.lock().unwrap().len();
    app.say_hi(Pool::Claude, &id).await.unwrap();
    assert_eq!(
        *fake.hello_requests.lock().unwrap(),
        vec![("alice-access".into(), "socks5://127.0.0.1:7891".into())]
    );
    // The usage, which shows the window only later, is left to the card.
    assert_eq!(fake.usage_tokens.lock().unwrap().len(), usages);
    assert_eq!(*fake.hello_accounts.lock().unwrap(), vec!["acct-alice"]);

    // A login Claude does not accept is renewed and says hi again.
    fake.hello_requests.lock().unwrap().clear();
    fake.hellos.lock().unwrap().push((401, Value::Null));
    fake.usage.lock().unwrap().push((401, Value::Null));
    app.say_hi(Pool::Claude, &id).await.unwrap();
    let tokens: Vec<String> = fake
        .hello_requests
        .lock()
        .unwrap()
        .iter()
        .map(|(token, _)| token.clone())
        .collect();
    assert_eq!(tokens, vec!["alice-access", "fresh-access"]);

    // Other failures are reported without retrying.
    fake.hello_requests.lock().unwrap().clear();
    fake.hellos
        .lock()
        .unwrap()
        .push((529, json!({"error": {"message": "Overloaded"}})));
    assert_eq!(
        app.say_hi(Pool::Claude, &id).await.unwrap_err(),
        "Claude message request failed: HTTP 529 Overloaded"
    );
    assert_eq!(fake.hello_requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn account_proxy_routes_imports_and_refreshes() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _location, fake) = app(&dir).await;
    let mut preferences = app.preferences().unwrap();
    preferences.account_proxy_url = "http://127.0.0.1:7890".into();
    app.save_preferences(preferences).await.unwrap();
    app.claude_import_login(json!({
        "access_token": "new-access", "refresh_token": "new-refresh",
        "expires_in": 3600,
        "account": {"uuid": "acct-new"}, "organization": {"uuid": "org-new"}
    }))
    .await
    .unwrap();
    // The import's requests take the shared proxy, which the new
    // account does not keep as its own.
    assert_eq!(
        *fake.profile_proxies.lock().unwrap(),
        vec!["http://127.0.0.1:7890"]
    );
    assert_eq!(
        *fake.usage_proxies.lock().unwrap(),
        vec!["http://127.0.0.1:7890"]
    );
    let account = app.claude_load().unwrap().accounts.remove(0);
    assert!(account.proxy_url.is_empty());
    app.refresh_usage(Pool::Claude, &account.id).await.unwrap();
    assert_eq!(
        fake.usage_proxies.lock().unwrap().last().unwrap(),
        "http://127.0.0.1:7890"
    );
    app.save_account_proxy(Pool::Claude, &account.id, "socks5://127.0.0.1:7891")
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &account.id).await.unwrap();
    assert_eq!(
        fake.usage_proxies.lock().unwrap().last().unwrap(),
        "socks5://127.0.0.1:7891"
    );
    let mut preferences = app.preferences().unwrap();
    preferences.account_proxy_url.clear();
    app.save_preferences(preferences).await.unwrap();
    app.refresh_usage(Pool::Claude, &account.id).await.unwrap();
    assert_eq!(
        fake.usage_proxies.lock().unwrap().last().unwrap(),
        "socks5://127.0.0.1:7891"
    );
}

#[tokio::test]
async fn browser_profile_uses_the_existing_account_proxy_with_rotated_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    // The currently selected account must not supply the new login's proxy.
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let alice = app.claude_load().unwrap().accounts[0].id.clone();
    app.save_account_proxy(Pool::Claude, &alice, "http://127.0.0.1:8001")
        .await
        .unwrap();
    let (oauth, account) = login("new", LATER);
    let (id, _) = app
        .claude_upsert(oauth, account, Origin::Export)
        .await
        .unwrap();
    app.save_account_proxy(Pool::Claude, &id, "socks5://127.0.0.1:8002")
        .await
        .unwrap();
    app.claude_import_login(json!({
        "access_token": "rotated-access", "refresh_token": "rotated-refresh",
        "expires_in": 3600,
        "account": {"uuid": "acct-new"}, "organization": {"uuid": "org-new"}
    }))
    .await
    .unwrap();
    assert_eq!(
        *fake.profile_proxies.lock().unwrap(),
        vec!["socks5://127.0.0.1:8002"]
    );
    assert_eq!(app.claude_load().unwrap().accounts.len(), 2);
}

#[tokio::test]
/// A login without oauthAccount whose tokens match a pool account is
/// that account: no profile request is needed to name it.
async fn importing_without_local_identity_reuses_the_account_its_tokens_match() {
    for retained in ["accessToken", "refreshToken"] {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = app(&dir).await;
        sign_in(&location, "new", LATER);
        app.claude_import_current().await.unwrap();
        let id = app.claude_load().unwrap().accounts[0].id.clone();
        app.save_account_proxy(Pool::Claude, &id, "http://127.0.0.1:8002")
            .await
            .unwrap();
        let (mut oauth, _) = login("new", LATER);
        let changed = if retained == "accessToken" {
            "refreshToken"
        } else {
            "accessToken"
        };
        oauth[changed] = "rotated".into();
        write_json(
            &location.credentials_file(),
            &json!({"claudeAiOauth": oauth}),
        )
        .unwrap();
        write_json(&location.global_config, &json!({})).unwrap();
        app.claude_import_current().await.unwrap();
        assert!(fake.profile_proxies.lock().unwrap().is_empty());
        let accounts = app.claude_load().unwrap().accounts;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].own.account["accountUuid"], "acct-new");
        assert_eq!(accounts[0].proxy_url, "http://127.0.0.1:8002");
    }
}

#[tokio::test]
async fn an_unknown_profile_does_not_use_another_accounts_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.save_account_proxy(Pool::Claude, &id, "http://127.0.0.1:8001")
        .await
        .unwrap();
    app.claude_import_login(json!({
        "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 3600
    }))
    .await
    .unwrap();
    assert_eq!(*fake.profile_proxies.lock().unwrap(), vec![""]);
}

/// A Claude Code login whose configuration does not name its account.
/// The accounts an earlier version kept in `claude-accounts.json` move
/// to the pool's file once, as they were, `oauth` as `document`; what
/// served the removed generations is left behind. The earlier file stays
/// as it was.
#[tokio::test]
async fn the_earlier_store_moves_to_the_pools_file_once() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let (oauth, account) = login("alice", LATER);
    let earlier = json!({"accounts": [{"id": "a", "label": "alice@example.com",
            "oauth": oauth, "account": account, "proxyUrl": "socks5://127.0.0.1:1080",
            "addedAt": 1, "updatedAt": 2, "usageRetryAt": 5, "rateLimits": 2,
            "renewal": "never", "firstRequestPending": true,
            "alternates": [oauth], "rejected": ["x"], "inLive": true}],
            "currentAccountId": "a", "settingsProvider": "p"});
    write_json(&data.join(EARLIER_STORE_FILE), &earlier).unwrap();
    let before = fs::read(data.join(EARLIER_STORE_FILE)).unwrap();
    let (app, _, _) = waiting_app(&dir);
    let store = app.claude_load().unwrap();
    let moved = &store.accounts[0];
    assert_eq!(moved.document, oauth);
    assert_eq!(moved.own.account, account);
    assert_eq!(moved.proxy_url, "socks5://127.0.0.1:1080");
    assert_eq!((moved.added_at, moved.updated_at), (1, 2));
    assert_eq!(
        (moved.own.usage_retry_at, moved.own.rate_limits),
        (Some(5), 2)
    );
    assert_eq!(moved.renewal, Some(Renewal::Never));
    assert!(moved.first_request_pending);
    assert_eq!(store.current_account_id.as_deref(), Some("a"));
    assert_eq!(store.settings_provider.as_deref(), Some("p"));
    let file: Value = serde_json::from_str(&store_text(&app)).unwrap();
    assert!(file["accounts"][0].get("alternates").is_none());
    assert_eq!(fs::read(data.join(EARLIER_STORE_FILE)).unwrap(), before);
    // Once: a pool emptied since is not filled again.
    app.ops(Pool::Claude).delete("a").await.unwrap();
    drop(app);
    let (app, _, _) = waiting_app(&dir);
    assert!(app.claude_load().unwrap().accounts.is_empty());
}

#[tokio::test]
async fn an_earlier_export_revives_only_a_signed_out_account() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _, _) = waiting_app(&dir);
    let (oauth, account) = login("alice", LATER);
    let export = json!({"claudeAiOauth": oauth, "oauthAccount": account}).to_string();
    app.claude_import_json(&export).await.unwrap();
    for (error, revived) in [
        ("Claude usage request failed: HTTP 500", false),
        (SIGN_IN_EXPIRED, true),
    ] {
        // The card holds a later generation, and an error.
        let mut store = app.claude_load().unwrap();
        store.accounts[0].document["accessToken"] = "alice-later".into();
        store.accounts[0].document["expiresAt"] = (LATER + 5).into();
        store.accounts[0].usage_error = Some(error.into());
        app.claude_save(&store).unwrap();
        app.claude_import_json(&export).await.unwrap();
        let stored = app.claude_load().unwrap().accounts[0].clone();
        if revived {
            assert_eq!(stored.document["accessToken"], "alice-access");
            assert_eq!(stored.usage_error, None);
        } else {
            assert_eq!(stored.document["accessToken"], "alice-later");
            assert_eq!(stored.usage_error.as_deref(), Some(error));
        }
    }
}

#[tokio::test]
async fn a_json_import_keeps_none_of_a_list_one_entry_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _, _) = waiting_app(&dir);
    let export = |user: &str| {
        let (oauth, account) = login(user, LATER);
        json!({"claudeAiOauth": oauth, "oauthAccount": account})
    };
    app.claude_import_json(&json!([export("alice"), export("bob")]).to_string())
        .await
        .unwrap();
    // Two cards hold alice's tokens, each with its own proxy.
    let mut store = app.claude_load().unwrap();
    store.accounts[1].document = store.accounts[0].document.clone();
    store.accounts[1].proxy_url = "http://127.0.0.1:8001".into();
    app.claude_save(&store).unwrap();
    let (nameless, _) = login("alice", LATER);
    let list = json!([export("carol"), {"claudeAiOauth": nameless}]);
    assert!(app.claude_import_json(&list.to_string()).await.is_err());
    assert_eq!(app.claude_load().unwrap().accounts.len(), 2);
}

#[test]
fn copool_opens_with_an_earlier_store_it_cannot_read() {
    for earlier in ["not json", r#"{"accounts": [{"id": 1}]}"#] {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join(EARLIER_STORE_FILE), earlier).unwrap();
        let (app, _, _) = waiting_app(&dir);
        assert!(app.claude_load().unwrap().accounts.is_empty());
        assert_eq!(
            fs::read_to_string(data.join(EARLIER_STORE_FILE)).unwrap(),
            earlier
        );
    }
}

/// The Claude pool as the store's suite drives it.
struct Fixture {
    app: Application,
    location: Location,
    fake: Arc<Fake>,
    _dir: tempfile::TempDir,
}
impl crate::login_store::suite::Fixture for Fixture {
    const POOL: Pool = Pool::Claude;
    const LIVE_UNREADABLE: &'static str = LIVE_UNREADABLE;
    async fn open(request_new_accounts: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (app, location, fake) = if request_new_accounts {
            app(&dir).await
        } else {
            waiting_app(&dir)
        };
        // A sign-in is named by its token response alone.
        fake.profile_fails
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Self {
            app,
            location,
            fake,
            _dir: dir,
        }
    }
    fn app(&self) -> &Application {
        &self.app
    }
    /// An export: `user`'s tokens, expiring at `expires` (seconds), and
    /// account.
    fn login(user: &str, access: &str, expires: i64) -> Value {
        let (mut oauth, account) = login(user, expires * 1000);
        oauth["accessToken"] = access.into();
        json!({"claudeAiOauth": oauth, "oauthAccount": account})
    }
    fn with_refresh(login: &Value, refresh: &str) -> Value {
        let mut login = login.clone();
        login["claudeAiOauth"]["refreshToken"] = refresh.into();
        login
    }
    fn without_expiry(login: &Value) -> Value {
        let mut login = login.clone();
        login["claudeAiOauth"]
            .as_object_mut()
            .unwrap()
            .remove("expiresAt");
        login
    }
    fn no_login() -> Value {
        json!({"claudeAiOauth": {"accessToken": "none", "refreshToken": ""}})
    }
    fn access(document: &Value) -> Option<String> {
        text(&document["claudeAiOauth"]["accessToken"])
    }
    fn renewed(_: &str) -> String {
        "fresh-access".into()
    }
    /// Claude's sign-in answers with a token response naming the account.
    fn browser_sign_in(login: &Value) -> Value {
        let oauth = &login["claudeAiOauth"];
        let account = &login["oauthAccount"];
        json!({
            "access_token": oauth["accessToken"], "refresh_token": oauth["refreshToken"],
            "expires_in": (expires_at(oauth) / 1000 - now_unix_seconds()).max(0),
            "account": {"uuid": account["accountUuid"], "email_address": account["emailAddress"]},
            "organization": {"uuid": account["organizationUuid"]}
        })
    }
    fn sign_in(&self, login: &Value) {
        fs::create_dir_all(&self.location.config_dir).unwrap();
        write_json(
            &self.location.credentials_file(),
            &json!({"claudeAiOauth": login["claudeAiOauth"]}),
        )
        .unwrap();
        write_account(&self.location, &login["oauthAccount"]).unwrap();
    }
    /// Claude Code's login, as an export.
    fn live(&self) -> Value {
        let document: Value =
            serde_json::from_slice(&fs::read(self.location.credentials_file()).unwrap()).unwrap();
        let config: Value =
            serde_json::from_slice(&fs::read(self.location.account_file()).unwrap()).unwrap();
        json!({"claudeAiOauth": document["claudeAiOauth"], "oauthAccount": config["oauthAccount"]})
    }
    fn corrupt_live(&self) {
        fs::write(self.location.credentials_file(), "not a login").unwrap();
    }
    fn delay_live_reads(&self, ms: u64) {
        self.app
            .claude
            .live_read_delay_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
    }
    fn meanwhile(&self, login: Value) {
        let location = self.location.clone();
        self.fake
            .refresh_hooks
            .lock()
            .unwrap()
            .push(Box::new(move || {
                write_json(
                    &location.credentials_file(),
                    &json!({"claudeAiOauth": login["claudeAiOauth"]}),
                )
                .unwrap();
                write_account(&location, &login["oauthAccount"]).unwrap();
            }));
    }
    fn renewals(&self) -> usize {
        self.fake.refreshed.lock().unwrap().len()
    }
    fn id_of(&self, user: &str) -> String {
        let email = format!("{user}@example.com");
        self.app
            .claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| text(&a.own.account["emailAddress"]).as_deref() == Some(email.as_str()))
            .unwrap()
            .id
    }
    /// The account's login, as an export.
    fn stored(&self, id: &str) -> Value {
        let account = self
            .app
            .claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .unwrap();
        json!({"claudeAiOauth": account.document, "oauthAccount": account.own.account})
    }
    fn current(&self) -> Option<String> {
        self.app.claude_load().unwrap().current_account_id
    }
}
crate::login_store::login_store_suite!(super::Fixture);

fn sign_in_unnamed(location: &Location, user: &str) {
    let (oauth, _) = login(user, LATER);
    fs::create_dir_all(&location.config_dir).unwrap();
    write_json(
        &location.credentials_file(),
        &json!({"claudeAiOauth": oauth}),
    )
    .unwrap();
    let _ = fs::remove_file(&location.global_config);
}

#[tokio::test]
async fn a_nameless_external_rotation_only_updates_the_profile_matched_card() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = app(&dir).await;
    // The rotation lasts longer than these logins.
    for user in ["new", "other"] {
        sign_in(&location, user, LATER - 1);
        app.claude_import_current().await.unwrap();
    }
    let mut preferences = app.preferences().unwrap();
    preferences.account_proxy_url = "http://127.0.0.1:8787".into();
    app.save_preferences(preferences).await.unwrap();
    fake.profile_proxies.lock().unwrap().clear();
    sign_in_unnamed(&location, "rotated");
    let cards = app.accounts(Pool::Claude).await.unwrap();
    for user in ["other", "new"] {
        let card = cards
            .iter()
            .find(|a| a.label == format!("{user}@example.com"))
            .unwrap();
        app.refresh_usage(Pool::Claude, &card.id).await.unwrap();
        let saved = app
            .claude_load()
            .unwrap()
            .accounts
            .into_iter()
            .find(|a| a.id == card.id)
            .unwrap();
        let expected = if user == "new" {
            "rotated-access"
        } else {
            "other-access"
        };
        assert_eq!(saved.document["accessToken"], expected);
        assert_eq!(fake.usage_tokens.lock().unwrap().last().unwrap(), expected);
    }
    let checked = fake.profile_proxies.lock().unwrap();
    assert!(!checked.is_empty());
    assert!(checked.iter().all(|p| p == "http://127.0.0.1:8787"));
}

#[tokio::test]
async fn cards_carry_the_access_token_expiry_in_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = waiting_app(&dir);
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(accounts[0].token_expires_at, Some(LATER / 1000));
}

#[tokio::test]
async fn an_unnamed_login_is_imported_and_probed_on_its_first_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in_unnamed(&location, "probe");
    assert_eq!(app.claude_import_current().await.unwrap(), 1);
    assert_eq!(
        app.accounts(Pool::Claude).await.unwrap()[0].label,
        "Claude Code"
    );
    app.claude_import_current().await.unwrap();
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(accounts.len(), 1);
    let card = &accounts[0];
    assert!(card.is_current && card.awaiting_first_request && card.email.is_none());
    assert!(fake.profile_proxies.lock().unwrap().is_empty());

    app.save_account_proxy(Pool::Claude, &card.id, "http://127.0.0.1:8003")
        .await
        .unwrap();
    app.release_new_accounts(Pool::Claude, Some(&card.id))
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &card.id).await.unwrap();
    assert_eq!(
        *fake.profile_proxies.lock().unwrap(),
        vec!["http://127.0.0.1:8003"]
    );
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(accounts.len(), 1);
    let card = &accounts[0];
    assert_eq!(card.label, "new@example.com");
    assert_eq!(card.plan_type.as_deref(), Some("max 5x"));
    assert!(card.is_current && card.usage.is_some());
}

#[tokio::test]
async fn a_probed_login_folds_into_the_pool_card_of_its_account() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    app.claude_import_login(json!({
        "access_token": "browser-access", "refresh_token": "browser-refresh",
        "expires_in": 3600,
        "account": {"uuid": "acct-new", "email_address": "new@example.com"},
        "organization": {"uuid": "org-new"}
    }))
    .await
    .unwrap();
    sign_in_unnamed(&location, "probe");
    app.claude_import_current().await.unwrap();
    assert_eq!(app.accounts(Pool::Claude).await.unwrap().len(), 2);
    app.release_new_accounts(Pool::Claude, None).await.unwrap();
    app.refresh_all(Pool::Claude).await.unwrap();
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].label, "new@example.com");
    assert!(accounts[0].is_current);
    // Switching to it later names the account in Claude Code again.
    let stored = app.claude_load().unwrap().accounts[0].clone();
    assert_eq!(stored.document["accessToken"], "probe-access");
    assert!(refreshes(&fake).is_empty());
}

#[tokio::test]
async fn a_new_account_contacts_upstream_only_after_a_manual_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, fake) = waiting_app(&dir);
    sign_in(&location, "alice", 1);
    app.claude_import_current().await.unwrap();
    app.claude_import_login(json!({
        "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 3600,
        "account": {"uuid": "acct-new", "email_address": "new@example.com"},
        "organization": {"uuid": "org-new"}
    }))
    .await
    .unwrap();
    // Neither import, nor the background refresh, reached upstream.
    app.refresh_recurring(Pool::Claude, 0).await.unwrap();
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(accounts.len(), 2);
    assert!(accounts
        .iter()
        .all(|a| a.awaiting_first_request && a.usage.is_none()));
    assert!(fake.refreshed.lock().unwrap().is_empty());
    assert!(fake.profile_proxies.lock().unwrap().is_empty());

    // A manual refresh releases one account, through its own proxy, and
    // learns the plan its profile was not asked for at sign-in.
    let new = accounts
        .iter()
        .find(|a| a.label == "new@example.com")
        .unwrap();
    app.save_account_proxy(Pool::Claude, &new.id, "socks5://127.0.0.1:8002")
        .await
        .unwrap();
    app.release_new_accounts(Pool::Claude, Some(&new.id))
        .await
        .unwrap();
    app.refresh_usage(Pool::Claude, &new.id).await.unwrap();
    assert_eq!(
        *fake.profile_proxies.lock().unwrap(),
        vec!["socks5://127.0.0.1:8002"]
    );
    let accounts = app.accounts(Pool::Claude).await.unwrap();
    let new = accounts.iter().find(|a| a.id == new.id).unwrap();
    assert!(!new.awaiting_first_request);
    assert!(new.usage.is_some());
    assert_eq!(new.plan_type.as_deref(), Some("max 5x"));
    assert!(accounts
        .iter()
        .any(|a| a.id != new.id && a.awaiting_first_request));
    assert!(fake.refreshed.lock().unwrap().is_empty());
}

#[test]
fn set_member_adds_to_empty_and_filled_objects() {
    let value = json!({"a": 1});
    assert_eq!(
        set_member("{}", "k", &value).unwrap(),
        "{\n  \"k\": {\n    \"a\": 1\n  }\n}"
    );
    let added = set_member("{\"x\": \"\\\"}\"}\n", "k", &json!(2)).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&added).unwrap(),
        json!({"x":"\"}","k":2})
    );
    assert!(set_member("[]", "k", &value).is_err());
}

#[test]
fn settings_that_bypass_the_account_login_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let location = Location::files(dir.path().to_path_buf());
    assert!(login_overrides(&location).is_empty());
    fs::write(
            dir.path().join("settings.json"),
            r#"{"apiKeyHelper":"","env":{"ANTHROPIC_BASE_URL":"https://gateway.invalid","ANTHROPIC_AUTH_TOKEN":"","CLAUDE_CODE_USE_BEDROCK":"0","DISABLE_TELEMETRY":"1"}}"#,
        )
        .unwrap();
    assert!(login_overrides(&location).is_empty());
}

/// Resolves the location for `vars` under a home holding `settings` as
/// the user settings.
fn resolved(vars: &[(&str, &str)], settings: Option<&str>) -> (Location, PathBuf) {
    let home = tempfile::tempdir().unwrap().keep();
    if let Some(settings) = settings {
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/settings.json"), settings).unwrap();
    }
    let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let env = move |name: &str| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    (Location::resolve(&env, home.clone(), true), home)
}
const CUSTOM: &str = "http://127.0.0.1:8787/anthropic";

#[test]
fn the_login_is_where_claude_code_keeps_it() {
    let (plain, home) = resolved(&[], None);
    assert_eq!(plain.keychain.as_deref(), Some("Claude Code-credentials"));
    assert!(plain.vault);
    assert_eq!(plain.config_dir, home.join(".claude"));
    assert_eq!(plain.storage_dir, home.join(".claude"));
    assert_eq!(plain.global_config, home.join(".claude.json"));
    assert_eq!(
        plain.credentials_file(),
        home.join(".claude/.credentials.json")
    );
    // An approved custom OAuth endpoint, from the settings or the
    // environment, names its own item; the global configuration stays.
    let approved = "https://claude.fedstart.com";
    let in_settings =
        |url: &str| format!(r#"{{"env":{{"CLAUDE_CODE_CUSTOM_OAUTH_URL":"{url}"}}}}"#);
    for (vars, settings) in [
        (vec![("CLAUDE_CODE_CUSTOM_OAUTH_URL", approved)], None),
        (vec![], Some(in_settings(approved))),
        (
            vec![],
            Some(in_settings("https://claude-staging.fedstart.com/")),
        ),
    ] {
        let (custom, home) = resolved(&vars, settings.as_deref());
        assert_eq!(
            custom.keychain.as_deref(),
            Some("Claude Code-custom-oauth-credentials")
        );
        assert_eq!(custom.global_config, home.join(".claude.json"));
        assert_eq!(
            custom.credentials_file(),
            home.join(".claude/.credentials.json")
        );
    }
    // Any other endpoint leaves Claude Code its file alone; the pool's
    // tokens still go to Copool's own Keychain item.
    for (vars, settings) in [
        (vec![], Some(in_settings(CUSTOM))),
        (vec![("CLAUDE_CODE_CUSTOM_OAUTH_URL", CUSTOM)], None),
        (
            vec![("CLAUDE_CODE_CUSTOM_OAUTH_URL", approved)],
            Some(in_settings(CUSTOM)),
        ),
    ] {
        let (file_only, home) = resolved(&vars, settings.as_deref());
        assert_eq!(file_only.keychain, None);
        assert!(file_only.vault);
        assert_eq!(
            file_only.credentials_file(),
            home.join(".claude/.credentials.json")
        );
    }
    // The settings override the environment, an empty value included;
    // other settings or unreadable ones change nothing.
    for (vars, settings) in [
        (
            vec![("CLAUDE_CODE_CUSTOM_OAUTH_URL", CUSTOM)],
            in_settings(""),
        ),
        (
            vec![],
            r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:8787/anthropic"}}"#.to_owned(),
        ),
        (
            vec![],
            r#"{"env":{"CLAUDE_CODE_CUSTOM_OAUTH_URL":7}}"#.to_owned(),
        ),
        (vec![], "not json".to_owned()),
    ] {
        let (location, _) = resolved(&vars, Some(&settings));
        assert_eq!(
            location.keychain.as_deref(),
            Some("Claude Code-credentials"),
            "{settings}"
        );
    }
    let (empty, _) = resolved(&[("CLAUDE_CODE_CUSTOM_OAUTH_URL", "")], None);
    assert_eq!(empty.keychain.as_deref(), Some("Claude Code-credentials"));
}

#[test]
fn the_pool_keeps_its_keychain_item_whatever_claude_code_uses() {
    let in_settings = format!(r#"{{"env":{{"CLAUDE_CODE_CUSTOM_OAUTH_URL":"{CUSTOM}"}}}}"#);
    let (keychain, _) = resolved(&[], None);
    let (file_only, _) = resolved(&[], Some(&in_settings));
    assert!(keychain.keychain.is_some() && file_only.keychain.is_none());
    let on_macos = cfg!(target_os = "macos");
    assert_eq!(uses_keychain_vault(&keychain), on_macos);
    // Claude Code's login in its file does not take the pool's item away.
    assert_eq!(uses_keychain_vault(&file_only), on_macos);
    assert!(!uses_keychain_vault(&Location::files("/smoke".into())));
    let home = tempfile::tempdir().unwrap();
    let elsewhere = Location::resolve(&|_| None, home.path().into(), false);
    assert!(!uses_keychain_vault(&elsewhere));
}

#[test]
fn the_keychain_hash_and_file_follow_the_configuration_and_storage_directories() {
    let dir = "/work/claude-config";
    let (custom_dir, _) = resolved(&[("CLAUDE_CONFIG_DIR", dir)], None);
    assert_eq!(
        custom_dir.keychain.as_deref(),
        Some("Claude Code-credentials-619df0a7")
    );
    assert_eq!(custom_dir.config_dir, PathBuf::from(dir));
    assert_eq!(custom_dir.storage_dir, PathBuf::from(dir));
    assert_eq!(
        custom_dir.global_config,
        PathBuf::from(dir).join(".claude.json")
    );
    let (both, _) = resolved(
        &[
            ("CLAUDE_CONFIG_DIR", dir),
            (
                "CLAUDE_CODE_CUSTOM_OAUTH_URL",
                "https://claude.fedstart.com",
            ),
        ],
        None,
    );
    assert_eq!(
        both.keychain.as_deref(),
        Some("Claude Code-custom-oauth-credentials-619df0a7")
    );
    // The secure storage directory, when set at all, replaces both.
    let (storage, _) = resolved(
        &[
            ("CLAUDE_CONFIG_DIR", dir),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/vault/store"),
        ],
        None,
    );
    assert_eq!(
        storage.keychain.as_deref(),
        Some("Claude Code-credentials-efec3556")
    );
    assert_eq!(storage.storage_dir, PathBuf::from("/vault/store"));
    assert_eq!(storage.config_dir, PathBuf::from(dir));
    let (unscoped, home) = resolved(
        &[
            ("CLAUDE_CONFIG_DIR", dir),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", ""),
        ],
        None,
    );
    assert_eq!(
        unscoped.keychain.as_deref(),
        Some("Claude Code-credentials")
    );
    assert_eq!(unscoped.storage_dir, home.join(".claude"));
    // Off macOS there is only the file; smoke tests stay in one directory.
    let home = tempfile::tempdir().unwrap();
    let env =
        |name: &str| (name == "CLAUDE_SECURESTORAGE_CONFIG_DIR").then(|| "/vault/store".to_owned());
    let linux = Location::resolve(&env, home.path().into(), false);
    assert_eq!(linux.keychain, None);
    assert!(!linux.vault);
    assert_eq!(
        linux.credentials_file(),
        PathBuf::from("/vault/store/.credentials.json")
    );
    let (smoke, _) = resolved(&[("COPOOL_CLAUDE_CONFIG_DIR", "/smoke")], None);
    assert_eq!(smoke, Location::files("/smoke".into()));
}

#[tokio::test]
async fn a_login_read_from_the_file_is_written_back_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let env = {
        let storage = storage.to_string_lossy().into_owned();
        move |name: &str| (name == "CLAUDE_SECURESTORAGE_CONFIG_DIR").then(|| storage.clone())
    };
    // Without the Keychain, as off macOS, the login is the storage file.
    let location = Location::resolve(&env, dir.path().into(), false);
    let (oauth, _) = login("alice", LATER);
    fs::create_dir_all(&storage).unwrap();
    write_json(
        &storage.join(".credentials.json"),
        &json!({"claudeAiOauth": oauth}),
    )
    .unwrap();
    let live = read_live(&location).await.unwrap().unwrap();
    assert_eq!(live.held, Held::File);
    assert_eq!(live.oauth(), Some(&oauth));
    // Where the Keychain item is missing, the file keeps receiving the
    // login; only a login read from the item, or none, goes to the item.
    let keychain = Location {
        keychain: Some("Claude Code-custom-oauth-credentials".into()),
        ..location
    };
    assert_eq!(written_keychain(&keychain, Some(Held::File)), None);
    for held in [Some(Held::Keychain), None] {
        assert_eq!(
            written_keychain(&keychain, held).map(String::as_str),
            Some("Claude Code-custom-oauth-credentials")
        );
    }
    assert_eq!(written_keychain(&Location::files(storage), None), None);
}

#[test]
fn the_account_follows_a_claude_json_inside_the_configuration_directory() {
    let dir = tempfile::tempdir().unwrap();
    let location = Location {
        config_dir: dir.path().join(".claude"),
        global_config: dir.path().join(".claude.json"),
        keychain: None,
        storage_dir: dir.path().join(".claude"),
        vault: false,
        custom_oauth_env: None,
    };
    assert_eq!(location.account_file(), location.global_config);
    fs::create_dir_all(&location.config_dir).unwrap();
    let inner = location.config_dir.join(".claude.json");
    fs::write(&inner, r#"{"oauthAccount":{"accountUuid":"a"}}"#).unwrap();
    assert_eq!(location.account_file(), inner);
    fs::write(
        &location.global_config,
        r#"{"oauthAccount":{"accountUuid":"b"}}"#,
    )
    .unwrap();
    assert_eq!(location.account_file(), location.global_config);
}

/// An Anthropic API stand-in: it records each request (path and query,
/// credential, betas, body) and answers the queued statuses, then 200.
async fn messages_upstream(
    statuses: Arc<Mutex<Vec<u16>>>,
) -> (String, Arc<Mutex<Vec<[String; 4]>>>) {
    use axum::{body::to_bytes, extract::Request, routing::any, Router};
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let router = Router::new().fallback(any(move |request: Request| {
            let (log, statuses) = (log.clone(), statuses.clone());
            async move {
                let (parts, body) = request.into_parts();
                let header = |name: &str| {
                    parts
                        .headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                };
                let credential = match header("x-api-key") {
                    key if !key.is_empty() => format!("x-api-key {key}"),
                    _ => header("authorization"),
                };
                let beta = header("anthropic-beta");
                let path = parts.uri.path_and_query().unwrap().to_string();
                let body = to_bytes(body, usize::MAX).await.unwrap();
                log.lock().unwrap().push([
                    path,
                    credential,
                    beta,
                    String::from_utf8_lossy(&body).into_owned(),
                ]);
                let status = {
                    let mut queued = statuses.lock().unwrap();
                    if queued.is_empty() { 200 } else { queued.remove(0) }
                };
                let body = if status == 200 {
                    json!({"type": "message", "content": [{"type": "text", "text": "hi"}]})
                } else {
                    json!({"type": "error", "error": {"type": "rate_limit_error", "message": "rate limit reached"}})
                };
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [("content-type", "application/json")],
                    body.to_string(),
                )
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, seen)
}
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
/// Two pool logins (Bob in use) and the Claude proxy started in front of
/// the stand-in, which Claude Code's official settings name.
async fn claude_proxy(
    dir: &tempfile::TempDir,
    statuses: Arc<Mutex<Vec<u16>>>,
) -> (
    Arc<Application>,
    Location,
    Arc<Fake>,
    Arc<Mutex<Vec<[String; 4]>>>,
    crate::ApiProxyStatus,
) {
    let (app, location, fake) = app(dir).await;
    let app = app.into_shared();
    let (base, seen) = messages_upstream(statuses).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    sign_in(&location, "bob", LATER);
    app.claude_import_current().await.unwrap();
    fs::write(
        settings_file(&location),
        json!({"env": {BASE_URL: base}}).to_string(),
    )
    .unwrap();
    let mut preferences = app.preferences().unwrap();
    preferences.claude_proxy_port = free_port();
    app.save_preferences(preferences).await.unwrap();
    let status = app.start_proxy(Pool::Claude).await.unwrap();
    (app, location, fake, seen, status)
}
async fn send(status: &crate::ApiProxyStatus, key: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1/messages?beta=true",
            status.base_url.as_deref().unwrap()
        ))
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "claude-code-20250219")
        .body(r#"{"model":"claude-sonnet-5-5","max_tokens":1,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    let code = response.status().as_u16();
    (code, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_claude_proxy_passes_messages_through_with_the_login_in_use() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _, _, seen, status) = claude_proxy(&dir, Default::default()).await;
    let key = status.api_key.clone().unwrap();
    assert!(status.running);
    // Claude Code takes `/anthropic` as its base URL and adds the rest.
    assert!(status.base_url.as_deref().unwrap().ends_with("/anthropic"));
    assert_eq!(
        app.proxy_status(Pool::Claude)
            .await
            .unwrap()
            .available_accounts,
        2
    );
    // Independent of the Codex proxy, which stays stopped.
    assert!(!app.proxy_status(Pool::Codex).await.unwrap().running);
    assert_ne!(
        app.proxy_status(Pool::Codex).await.unwrap().api_key,
        Some(key.clone())
    );
    let (code, body) = send(&status, &key).await;
    assert_eq!(
        (code, body["content"][0]["text"].as_str()),
        (200, Some("hi"))
    );
    let [path, credential, beta, body] = seen.lock().unwrap()[0].clone();
    assert_eq!(path, "/v1/messages?beta=true");
    assert_eq!(credential, "Bearer bob-access");
    assert_eq!(beta, "claude-code-20250219,oauth-2025-04-20");
    assert!(body.contains("claude-sonnet-5-5"));
    assert_eq!(
        app.proxy_status(Pool::Claude)
            .await
            .unwrap()
            .active_account_id
            .as_deref(),
        Some("acct-bob")
    );
    // A wrong key reaches no account.
    assert_eq!(send(&status, "sk-wrong").await.0, 401);
    assert_eq!(seen.lock().unwrap().len(), 1);
    app.stop_proxy(Pool::Claude).await.unwrap();
}

#[tokio::test]
async fn a_rate_limited_login_fails_over_and_claude_code_follows_unless_hosting_only() {
    let dir = tempfile::tempdir().unwrap();
    let statuses = Arc::new(Mutex::new(vec![429]));
    let (app, location, _, seen, status) = claude_proxy(&dir, statuses.clone()).await;
    let key = status.api_key.clone().unwrap();
    assert_eq!(send(&status, &key).await.0, 200);
    let credentials: Vec<String> = seen.lock().unwrap().iter().map(|r| r[1].clone()).collect();
    assert_eq!(credentials, ["Bearer bob-access", "Bearer alice-access"]);
    // As the Codex proxy's account, the one serving becomes Claude Code's.
    assert_eq!(live_access(&location), "alice-access");
    // Bob cools down; Alice keeps serving.
    assert_eq!(send(&status, &key).await.0, 200);
    assert_eq!(
        seen.lock().unwrap().last().unwrap()[1],
        "Bearer alice-access"
    );
    // Hosting the API only, the proxy leaves Claude Code's login alone.
    let mut preferences = app.preferences().unwrap();
    preferences.host_api_only = true;
    app.save_preferences(preferences).await.unwrap();
    sign_in(&location, "bob", LATER);
    app.accounts(Pool::Claude).await.unwrap();
    assert_eq!(send(&status, &key).await.0, 200);
    assert_eq!(live_access(&location), "bob-access");
    app.stop_proxy(Pool::Claude).await.unwrap();
}

#[tokio::test]
async fn a_refused_login_is_renewed_through_the_pool_and_retried() {
    let dir = tempfile::tempdir().unwrap();
    let statuses = Arc::new(Mutex::new(vec![401]));
    let (app, location, fake, seen, status) = claude_proxy(&dir, statuses).await;
    let key = status.api_key.clone().unwrap();
    assert_eq!(send(&status, &key).await.0, 200);
    let credentials: Vec<String> = seen.lock().unwrap().iter().map(|r| r[1].clone()).collect();
    assert_eq!(credentials, ["Bearer bob-access", "Bearer fresh-access"]);
    assert_eq!(*fake.refreshed.lock().unwrap(), ["bob-refresh"]);
    // The pool keeps the renewal, and Claude Code's login with it.
    assert_eq!(live_access(&location), "fresh-access");
    let bob = app
        .claude_load()
        .unwrap()
        .accounts
        .into_iter()
        .find(|a| a.label.contains("bob"))
        .unwrap();
    assert_eq!(bob.document["accessToken"], "fresh-access");
    // Another request refused with the same token gets the renewal,
    // without renewing again.
    let login = app.claude_renew_login(&bob.id, "bob-access").await.unwrap();
    assert_eq!(login["claudeAiOauth"]["accessToken"], "fresh-access");
    assert_eq!(*fake.refreshed.lock().unwrap(), ["bob-refresh"]);
    // A refused token is renewed only as the account's choice allows.
    app.set_renewal(Pool::Claude, &bob.id, "never")
        .await
        .unwrap();
    let error = app
        .claude_renew_login(&bob.id, "fresh-access")
        .await
        .unwrap_err();
    assert_eq!(error, RENEWAL_OFF);
    assert_eq!(fake.refreshed.lock().unwrap().len(), 1);
    app.stop_proxy(Pool::Claude).await.unwrap();
}

#[tokio::test]
async fn remote_servers_get_the_official_logins_and_their_renewals_come_back() {
    let dir = tempfile::tempdir().unwrap();
    let (app, location, _) = app(&dir).await;
    sign_in(&location, "alice", LATER);
    app.claude_import_current().await.unwrap();
    // A login not yet released stays with the app.
    let mut store = app.claude_load().unwrap();
    store.accounts[0].first_request_pending = true;
    app.claude_save(&store).unwrap();
    assert!(app.claude_remote_payload().is_err());
    store.accounts[0].first_request_pending = false;
    app.claude_save(&store).unwrap();
    let payload: Value = serde_json::from_slice(&app.claude_remote_payload().unwrap()).unwrap();
    let accounts = payload["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(
        accounts[0]["authJson"]["claudeAiOauth"]["accessToken"],
        "alice-access"
    );
    assert!(accounts[0].get("copoolCurrent").is_none());
    // The daemon renewed the login: the pool takes the later generation.
    let mut remote = payload.clone();
    remote["accounts"][0]["authJson"]["claudeAiOauth"] = json!({
        "accessToken": "remote-access", "refreshToken": "remote-refresh", "expiresAt": LATER + 1
    });
    app.claude_adopt_remote_logins(&remote).await.unwrap();
    assert_eq!(
        app.claude_load().unwrap().accounts[0].document["accessToken"],
        "remote-access"
    );
    // An older one is not.
    remote["accounts"][0]["authJson"]["claudeAiOauth"] = json!({
        "accessToken": "stale-access", "refreshToken": "stale-refresh", "expiresAt": 1
    });
    app.claude_adopt_remote_logins(&remote).await.unwrap();
    assert_eq!(
        app.claude_load().unwrap().accounts[0].document["accessToken"],
        "remote-access"
    );
    // The single renewal allowed, used by the daemon, is used here too,
    // and the next upload says so.
    let id = app.claude_load().unwrap().accounts[0].id.clone();
    app.set_renewal(Pool::Claude, &id, "once").await.unwrap();
    remote["accounts"][0]["renewal"] = "never".into();
    remote["accounts"][0]["authJson"]["claudeAiOauth"] = json!({
        "accessToken": "renewed-access", "refreshToken": "renewed-refresh", "expiresAt": LATER + 2
    });
    app.claude_adopt_remote_logins(&remote).await.unwrap();
    assert_eq!(
        app.claude_load().unwrap().accounts[0].renewal,
        Some(Renewal::Never)
    );
    let payload: Value = serde_json::from_slice(&app.claude_remote_payload().unwrap()).unwrap();
    assert_eq!(payload["accounts"][0]["renewal"], "never");
}
