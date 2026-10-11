//! Original SwiftNativeProxyRuntimeServiceTests against the shared Rust engine.
#![allow(non_snake_case)]
use super::*;
#[test]
fn testParsesCodexVersionOnlyFromRecognizedUserAgentProducts() {
    assert_eq!(
        parse_codex_version_from_user_agent(
            "codex_exec/7.8.9 (Mac OS 26.0.1; arm64) Apple_Terminal/464"
        ),
        Some("7.8.9")
    );
    assert_eq!(
        parse_codex_version_from_user_agent("codex_cli_rs/6.7.8 (Mac OS 26.0.1; arm64)"),
        Some("6.7.8")
    );
    assert_eq!(
        parse_codex_version_from_user_agent("openai-python/1.101.0 Python/3.13"),
        None
    );
    assert_eq!(
        parse_codex_version_from_user_agent("codex_exec/not-a-version"),
        None
    );
}
#[test]
fn testUpstreamVersionPrefersExplicitHeaderThenCodexUserAgentThenFallback() {
    assert_eq!(
        resolve_codex_client_version(Some("9.9.9"), "codex_exec/7.8.9"),
        "9.9.9"
    );
    assert_eq!(
        resolve_codex_client_version(None, "codex_exec/7.8.9"),
        "7.8.9"
    );
    assert_eq!(
        resolve_codex_client_version(None, "openai-python/1.101.0"),
        CODEX_CLIENT_VERSION
    );
}
#[test]
fn testNormalizesReasoningSummaryForUpstream() {
    for (input, expected) in [
        (Some("none"), "auto"),
        (Some("  NONE "), "auto"),
        (None, "auto"),
        (Some("concise"), "concise"),
    ] {
        assert_eq!(normalized_reasoning_summary(input), expected);
    }
}
#[test]
fn testNormalizesReasoningEffortForUpstream() {
    for (input, model, expected) in [
        (Some("none"), Some("gpt-5.1-codex-max"), "medium"),
        (Some("HIGH"), None, "high"),
        (Some("xhigh"), Some("gpt-5.3-codex"), "xhigh"),
        (Some("minimal"), Some("gpt-5.3-codex"), "medium"),
        (Some("none"), Some("gpt-4.1"), "none"),
        (Some("unexpected"), None, "none"),
        (None, None, "none"),
    ] {
        assert_eq!(normalized_reasoning_effort(input, model), expected);
    }
}
#[test]
fn testResolvesUpstreamRouteFamilyByModel() {
    for model in [
        "gpt-5.4",
        "gpt-5-4",
        "gpt-5-codex-mini",
        "gpt-5",
        "gpt-5-mini",
        "gpt-5.2",
    ] {
        assert!(matches!(
            resolve_upstream_route_family(model),
            UpstreamRouteFamily::Codex
        ));
    }
}
#[test]
fn testMapsDisplayModelNamesToUpstream() {
    for (input, expected) in [
        ("GPT-5.4", "gpt-5.4"),
        ("GPT-5.4-Mini", "gpt-5.4-mini"),
        ("GPT-5.3-Codex", "gpt-5.3-codex"),
    ] {
        assert_eq!(map_client_model_to_upstream(input).unwrap(), expected);
    }
}
#[test]
fn testMapsDisplayModelAliasNamesToUpstream() {
    for (input, expected) in [
        ("GPT-5.4-Low", "gpt-5.4"),
        ("GPT-5.4-High", "gpt-5.4"),
        ("GPT-5.4-Mini-High", "gpt-5.4-mini"),
        ("GPT-5.4-Mini-xHigh", "gpt-5.4-mini"),
        ("GPT-5.3-Codex-Medium", "gpt-5.3-codex"),
    ] {
        assert_eq!(map_client_model_to_upstream(input).unwrap(), expected);
    }
}
#[test]
fn testNormalizesUpstreamModelsForClientDisplay() {
    for (input, expected) in [
        ("gpt-5", "GPT-5"),
        ("gpt-5.3-codex", "GPT-5.3-Codex"),
        ("gpt-5-4", "GPT-5.4"),
        ("gpt-5-4-mini", "GPT-5.4-Mini"),
        ("gpt-5-4-xhigh", "GPT-5.4-xHigh"),
        ("gpt5.4-2026-03-09", "GPT-5.4-2026-03-09"),
    ] {
        assert_eq!(normalize_model_for_client(input), expected);
    }
}
#[test]
fn testClientVisibleModelsIncludeReasoningAliasNames() {
    for model in [
        "GPT-5.5",
        "GPT-5.5-High",
        "GPT-5.4-Low",
        "GPT-5.4-High",
        "GPT-5.4-Mini-High",
        "GPT-5.4-Mini-xHigh",
        "GPT-5.3-Codex-xHigh",
        "GPT-5.3-Codex-Medium",
    ] {
        assert!(MODELS.contains(&model), "{model}");
    }
}
#[test]
fn testResolvesUpstreamBaseURLForBothRouteFamilies() {
    assert_eq!(
        resolve_upstream_base_url("https://chatgpt.com", UpstreamRouteFamily::Codex),
        "https://chatgpt.com/backend-api/codex"
    );
    assert_eq!(
        resolve_upstream_base_url("https://chatgpt.com", UpstreamRouteFamily::General),
        "https://chatgpt.com/backend-api"
    );
    assert_eq!(
        resolve_upstream_base_url(
            "https://chatgpt.com/backend-api/codex/responses",
            UpstreamRouteFamily::Codex
        ),
        "https://chatgpt.com/backend-api/codex"
    );
    assert_eq!(
        resolve_upstream_base_url(
            "https://chatgpt.com/backend-api/responses",
            UpstreamRouteFamily::General
        ),
        "https://chatgpt.com/backend-api"
    );
}
#[test]
fn testResponsesNormalizesStringInputToMessageArray() {
    let (p, stream) = normalize_openai_responses_request(
        json!({"model":"gpt-5.4","input":"reply with exactly OK","stream":false}),
    )
    .unwrap();
    assert!(!stream);
    assert_eq!(p["model"], "gpt-5.4");
    assert_eq!(p["stream"], true);
    assert_eq!(p["input"].as_array().unwrap().len(), 1);
    let i = &p["input"][0];
    assert_eq!(i["type"], "message");
    assert_eq!(i["role"], "user");
    assert_eq!(i["content"].as_array().unwrap().len(), 1);
    assert_eq!(i["content"][0]["type"], "input_text");
    assert_eq!(i["content"][0]["text"], "reply with exactly OK");
}
#[test]
fn testResponsesNormalizationDropsUnsupportedForwardingFields() {
    let (p,_)=normalize_openai_responses_request(json!({"model":"gpt-5.4","input":[{"role":"user","content":[{"type":"input_text","text":"hello"}]}],"prompt_cache_key":"factory-droid","prompt_cache_retention":"24h","safety_identifier":"user-123","service_tier":"auto"})).unwrap();
    for key in [
        "prompt_cache_key",
        "prompt_cache_retention",
        "safety_identifier",
        "service_tier",
    ] {
        assert!(p.get(key).is_none());
    }
}
#[test]
fn testResponsesNormalizationDropsMaxOutputTokensAndTemperature() {
    let (p, _) = normalize_openai_responses_request(
        json!({"model":"gpt-5.4","input":"hello","max_output_tokens":32,"temperature":0.1}),
    )
    .unwrap();
    assert!(p.get("max_output_tokens").is_none());
    assert!(p.get("temperature").is_none());
}
#[test]
fn testResponsesNormalizationInjectsReasoningEffortFromModelAlias() {
    let (p, _) =
        normalize_openai_responses_request(json!({"model":"GPT-5.4-High","input":"hello"}))
            .unwrap();
    assert_eq!(p["reasoning"]["effort"], "high");
    assert_eq!(p["reasoning"]["summary"], "auto");
}
#[test]
fn testResponsesNormalizationKeepsExplicitReasoningEffortOverAliasDefault() {
    let (p, _) = normalize_openai_responses_request(
        json!({"model":"GPT-5.4-High","input":"hello","reasoning":{"effort":"low"}}),
    )
    .unwrap();
    assert_eq!(p["reasoning"]["effort"], "low");
}
#[test]
fn testChatConversionInjectsReasoningEffortFromModelAlias() {
    let (p, _) = convert_openai_chat_request_to_codex(
        &json!({"model":"GPT-5.3-Codex-High","messages":[{"role":"user","content":"hello"}]}),
    )
    .unwrap();
    assert_eq!(p["reasoning"]["effort"], "high");
}
#[test]
fn testChatConversionMapsJSONResponseFormatToTextFormatType() {
    let(p,_)=convert_openai_chat_request_to_codex(&json!({"model":"GPT-5.4","messages":[{"role":"user","content":"return valid json"}],"response_format":{"type":"json_object"}})).unwrap();
    assert_eq!(p["text"]["format"]["type"], "json_object");
}
#[test]
fn testStreamingChatTranslatorEmitsChunksIncrementally() {
    let mut decoder = SseDecoder::default();
    let mut state = ChatStreamState::default();
    let first=b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\",\"created_at\":1,\"model\":\"gpt-5\"}}\n\nevent: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel";
    let chunks: Vec<_> = decoder
        .push(first)
        .iter()
        .flat_map(|e| translate_sse_event_to_chat_chunk(e, &mut state))
        .collect();
    assert!(chunks.is_empty());
    let mut events=decoder.push(b"lo\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"created_at\":1,\"model\":\"gpt-5\",\"status\":\"completed\"}}\n\n");
    events.extend(decoder.finish());
    let chunks: Vec<_> = events
        .iter()
        .flat_map(|e| translate_sse_event_to_chat_chunk(e, &mut state))
        .collect();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0]["choices"][0]["delta"]["content"], "Hello");
    assert_eq!(chunks[1]["choices"][0]["finish_reason"], "stop");
}
#[test]
fn testExtractCompletedResponseBackfillsOutputFromOutputItemDoneEvent() {
    let bytes=b"event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"msg_123\",\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"OK\"}]},\"output_index\":1,\"sequence_number\":9}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n";
    assert_eq!(
        extract_completed_response_from_sse(bytes).unwrap()["output"][0]["content"][0]["text"],
        "OK"
    );
}
#[test]
fn testPayloadOversizeDetectionFromContentLengthHeader() {
    let raw=format!("POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{{}}",DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES+1);
    assert!(is_payload_oversized(
        raw.as_bytes(),
        DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES
    ));
}
#[test]
fn testPayloadOversizeDetectionFromBufferedBytes() {
    assert!(is_payload_oversized(
        &vec![65; DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES + 1],
        DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES
    ));
}
#[test]
fn testPayloadOversizeDoesNotTriggerUnderLimit() {
    let raw = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{{}}",
        DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES - 128
    );
    assert!(!is_payload_oversized(
        raw.as_bytes(),
        DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES
    ));
}

fn stored_candidate(id: &str, workspace: &str, added: i64) -> Value {
    json!({"id":id,"label":format!("Account {id}"),"accountId":workspace,"authJson":{"tokens":{"access_token":format!("token-{id}"),"id_token":"synthetic","account_id":workspace}},"addedAt":added,"updatedAt":added})
}
fn desktop_context(store: Value) -> (tempfile::TempDir, ProxyContext) {
    let dir = tempfile::tempdir().unwrap();
    let store: crate::models::AccountsStore = serde_json::from_value(store).unwrap();
    save_store_to_path(&account_store_path_from_data_dir(dir.path()), &store).unwrap();
    let mut storage = new_proxy_storage_context(
        dir.path().to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(())),
        false,
    );
    storage.prefer_local_selection = true;
    let context = ProxyContext {
        storage,
        api_key: Arc::new(RwLock::new("test".into())),
        upstream_base_url: "http://127.0.0.1:1".into(),
        client: reqwest::Client::new(),
        shared: Arc::new(tokio::sync::Mutex::new(Default::default())),
    };
    (dir, context)
}
#[tokio::test]
async fn testLocalCandidatesKeepIndependentAccountProxies() {
    let mut a = stored_candidate("a", "a", 1);
    let mut b = stored_candidate("b", "b", 2);
    a["proxyURL"] = "http://127.0.0.1:8080".into();
    b["proxyURL"] = "socks5://127.0.0.1:1080".into();
    let (_dir, c) = desktop_context(json!({"accounts":[a,b]}));
    let candidates = load_proxy_candidates(&c.storage).await.unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|c| c.proxy_url.as_str())
            .collect::<Vec<_>>(),
        ["http://127.0.0.1:8080", "socks5://127.0.0.1:1080"]
    );
}
#[tokio::test]
async fn testLoadCandidatesPrefersCurrentSelectionWhenUsageIsUnavailable() {
    let (_dir, c) = desktop_context(
        json!({"accounts":[stored_candidate("a","acct-a",1),stored_candidate("b","acct-b",2)],"currentAccountId":"b","currentSelection":{"cardId":"acct-b","selectedAt":2,"sourceDeviceId":"macos-local"}}),
    );
    let v = load_proxy_candidates(&c.storage).await.unwrap();
    assert_eq!(
        v.iter().map(|c| c.account_id.as_str()).collect::<Vec<_>>(),
        ["acct-b", "acct-a"]
    );
}
#[tokio::test]
async fn testLoadCandidatesPrefersCurrentCardIDOverStaleSelection() {
    let (_dir, c) = desktop_context(
        json!({"accounts":[stored_candidate("a","acct-a",1),stored_candidate("b","acct-b",2)],"currentAccountId":"b","currentSelection":{"cardId":"acct-a","selectedAt":2,"sourceDeviceId":"macos-local"}}),
    );
    let v = load_proxy_candidates(&c.storage).await.unwrap();
    assert_eq!(
        v.iter().map(|c| c.account_id.as_str()).collect::<Vec<_>>(),
        ["acct-b", "acct-a"]
    );
}
#[tokio::test]
async fn testLoadCandidatesFallsBackToCurrentCardWhenSelectionIdentityIsAmbiguous() {
    let (_dir, c) = desktop_context(
        json!({"accounts":[stored_candidate("a","shared-account",1),stored_candidate("b","shared-account",2)],"currentAccountId":"b","currentSelection":{"cardId":"shared-account","selectedAt":2,"sourceDeviceId":"macos-local"}}),
    );
    let v = load_proxy_candidates(&c.storage).await.unwrap();
    assert_eq!(
        v.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["b", "a"]
    );
}
#[tokio::test]
async fn testLoadCandidatesUsesAddedAtAsStableTieBreakerForEqualScores() {
    let (_dir, c) = desktop_context(
        json!({"accounts":[stored_candidate("later","acct-later",20),stored_candidate("earlier","acct-earlier",10)]}),
    );
    let v = load_proxy_candidates(&c.storage).await.unwrap();
    assert_eq!(
        v.iter().map(|c| c.account_id.as_str()).collect::<Vec<_>>(),
        ["acct-earlier", "acct-later"]
    );
}
#[tokio::test]
async fn testCurrentCandidatesPrefersStickyAccountAfterSuccessfulSelection() {
    let (_dir, c) = desktop_context(
        json!({"accounts":[stored_candidate("a","acct-a",1),stored_candidate("b","acct-b",2)]}),
    );
    let candidates = load_proxy_candidates(&c.storage).await.unwrap();
    record_successful_candidate(&c, &candidates[1]).await;
    let v = order_proxy_candidates_for_runtime(&c, candidates).await;
    assert_eq!(
        v.iter().map(|c| c.account_id.as_str()).collect::<Vec<_>>(),
        ["acct-b", "acct-a"]
    );
}
#[tokio::test]
async fn testCurrentCandidatesPrefersManualSelectionOverStickyAccount() {
    let (_dir, c) = desktop_context(
        json!({"accounts":[stored_candidate("a","acct-a",1),stored_candidate("b","acct-b",2)],"currentAccountId":"b"}),
    );
    c.shared.lock().await.sticky_account_id = Some("a".into());
    let candidates = load_proxy_candidates(&c.storage).await.unwrap();
    let v = order_proxy_candidates_for_runtime(&c, candidates).await;
    assert_eq!(
        v.iter().map(|c| c.account_id.as_str()).collect::<Vec<_>>(),
        ["acct-b", "acct-a"]
    );
}
#[tokio::test]
async fn testCurrentCandidatesSkipsAccountInCooldownWindow() {
    let (_dir, c) = desktop_context(
        json!({"accounts":[stored_candidate("a","acct-a",1),stored_candidate("b","acct-b",2)]}),
    );
    let candidates = load_proxy_candidates(&c.storage).await.unwrap();
    mark_candidate_cooldown(&c, &candidates[0], RetryFailureCategory::RateLimited).await;
    let v = order_proxy_candidates_for_runtime(&c, candidates).await;
    assert_eq!(
        v.iter().map(|c| c.account_id.as_str()).collect::<Vec<_>>(),
        ["acct-b"]
    );
}
