use crate::config::{SearchCfg, SpeechOutputCfg};
use crate::llm::LlmClient;
use serde_json::Value;

const KNOWLEDGE_CHECK_SENTINEL: &str =
    "I do not have enough up-to-date information to answer this.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct StubLlm {
        out: String,
    }

    impl crate::llm::LlmClient for StubLlm {
        fn classify_intent(
            &self,
            _text: &str,
            _commands: &[crate::intent::IntentCommand],
        ) -> Result<crate::llm::LlmIntent, String> {
            Err("not used".into())
        }

        fn summarize_search(&self, _query: &str, _snippets: &[String]) -> Result<String, String> {
            Err("not used".into())
        }

        fn answer_short(&self, _prompt: &str) -> Result<String, String> {
            Ok(self.out.clone())
        }

        fn tts(&self, _text: &str) -> Result<Vec<u8>, String> {
            Err("not used".into())
        }
    }

    #[test]
    fn knowledge_check_exact_sentinel_triggers_unknown() {
        let llm: Arc<dyn crate::llm::LlmClient> = Arc::new(StubLlm {
            out: KNOWLEDGE_CHECK_SENTINEL.to_string(),
        });
        let res = answer_with_llm_if_known("who won f1 2025", &llm).unwrap();
        assert!(matches!(res, KnownOrUnknown::Unknown));
    }

    #[test]
    fn knowledge_check_vague_disclaimer_is_treated_as_known_not_unknown() {
        let llm: Arc<dyn crate::llm::LlmClient> = Arc::new(StubLlm {
            out: "I don't have real-time data".to_string(),
        });
        let res = answer_with_llm_if_known("today's weather", &llm).unwrap();
        assert!(matches!(res, KnownOrUnknown::Known(_)));
    }
}

enum KnownOrUnknown {
    Known(String),
    Unknown,
}

fn answer_with_llm_if_known(
    query: &str,
    llm: &std::sync::Arc<dyn LlmClient>,
) -> Result<KnownOrUnknown, String> {
    // Stage 1: strict knowledge check.
    // Must return the exact sentinel string if it cannot answer confidently from static knowledge.
    let prompt = format!(
        "You are an AI assistant named Bumblebee, running on arch linux (just like siri for mac).\n\nAnswer the user ONLY IF you are certain the answer is:\n- Not time-sensitive\n- Not dependent on real-time data\n- Not dependent on events after your training cutoff\n- Not dependent on current news, stock prices, sports results, weather, or recent events\n\nIf you can answer confidently from static knowledge, give the answer.\n\nIf you cannot answer confidently, respond with EXACTLY this sentence and nothing else:\n\n\"{}\"\n\nUser question:\n{}\n\nImportant: Never mention knowledge cutoff, training data, or that you are an AI language model.",
        KNOWLEDGE_CHECK_SENTINEL,
        query
    );

    let out = llm.answer_short(&prompt)?;
    if out.trim() == KNOWLEDGE_CHECK_SENTINEL {
        return Ok(KnownOrUnknown::Unknown);
    }

    let ans = out.trim();
    if ans.is_empty() {
        return Ok(KnownOrUnknown::Unknown);
    }
    Ok(KnownOrUnknown::Known(ans.to_string()))
}

fn answer_with_tavily(
    query: &str,
    cfg: &SearchCfg,
    llm: &std::sync::Arc<dyn LlmClient>,
) -> Result<String, String> {
    // Stage 2: Tavily -> facts-only Mistral compose.
    let facts = tavily_search(query, cfg.timeout_ms, cfg.country.as_deref())?;

    let prompt = format!(
        "User question:\n{}\n\nRetrieved web information:\n{}\n\nAnswer the question clearly and concisely using ONLY the information above.\nIf the information is insufficient or contradictory, say \"I don’t know.\"\n\nImportant: Never mention knowledge cutoff, training data, or that you are an AI language model.",
        query,
        facts.facts_text
    );

    llm.answer_short(&prompt)
}

#[derive(Debug, Clone)]
pub struct TavilyResult {
    pub raw: Value,
    pub facts_text: String,
}

pub fn search_and_summarize_async(
    question: String,
    search_cfg: SearchCfg,
    ui_enabled: bool,
    ui_timeout_ms: u64,
    tts: SpeechOutputCfg,
    llm: std::sync::Arc<dyn LlmClient>,
) {
    if !search_cfg.enabled {
        return;
    }

    std::thread::spawn(move || {
        let answer_timeout_ms = ui_timeout_ms.max(15_000);

        // For web results, abort early if offline.
        // Important: do not call Tavily and do not fall back to any other web flow.
        if !crate::net::has_internet(800) {
            if ui_enabled {
                crate::ui::notify_answer(
                    ui_enabled,
                    answer_timeout_ms,
                    "Btw",
                    "No internet connection. Cannot fetch web results.",
                );
            }
            return;
        }

        // Strict 2-stage gating:
        // 1) Ask LLM to answer only if it is certain (else return exact sentinel)
        // 2) Only if sentinel, call Tavily and then ask LLM again using ONLY retrieved info
        let (final_answer_res, source_label) = match answer_with_llm_if_known(&question, &llm) {
            Ok(KnownOrUnknown::Known(ans)) => (Ok(ans), "mistral"),
            Ok(KnownOrUnknown::Unknown) => (answer_with_tavily(&question, &search_cfg, &llm), "tavily"),
            Err(e) => (Err(e), "tavily"),
        };

        match final_answer_res {
            Ok(answer) => {
                if ui_enabled {
                    let ui_text = format!("{}\n\n:source: {}", answer, source_label);

                    if source_label == "tavily" {
                        let google_url = format!(
                            "https://www.google.com/search?q={}",
                            urlencoding::encode(&question)
                        );
                        crate::ui::notify_answer_with_open_in_browser(
                            ui_enabled,
                            answer_timeout_ms,
                            "Btw",
                            &ui_text,
                            &google_url,
                        );
                    } else {
                        crate::ui::notify_answer(ui_enabled, answer_timeout_ms, "Btw", &ui_text);
                    }
                }
                // Speak the *Mistral-produced* answer only. Never speak raw Tavily facts.
                let mut tts_force = tts.clone();
                tts_force.enabled = true;
                crate::tts::speak_async(answer, tts_force);
            }
            Err(e) => {
                eprintln!("TAVILY error: {}", e);
                let msg = "I couldn’t find reliable information.".to_string();
                if ui_enabled {
                    let ui_text = format!("{}\n\n:source: {}", msg, source_label);
                    crate::ui::notify_answer(ui_enabled, answer_timeout_ms, "Btw", &ui_text);
                }
                let mut tts_force = tts;
                tts_force.enabled = true;
                crate::tts::speak_async(msg, tts_force);
            }
        }
    });
}

pub fn tavily_search(query: &str, timeout_ms: u64, country: Option<&str>) -> Result<TavilyResult, String> {
    let api_key = std::env::var("TAVILY_API_KEY")
        .map_err(|_| "missing TAVILY_API_KEY".to_string())?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .build()
        .map_err(|e| format!("client build: {}", e))?;

    let url = "https://api.tavily.com/search";

    // Match required request shape:
    // - Use `Authorization: Bearer <key>` header
    // - Fields: query, include_answer="basic", search_depth="basic", country
    let mut req_body = serde_json::json!({
        "query": query,
        "include_answer": "basic",
        "search_depth": "basic"
    });

    if let Some(country) = country {
        if !country.trim().is_empty() {
            req_body["country"] = serde_json::Value::String(country.trim().to_string());
        }
    }

    let resp = client
        .post(url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", api_key))
        .json(&req_body)
        .send()
        .map_err(|e| {
            format!(
                "http error (tavily search): connect={} timeout={} source={}",
                e.is_connect(),
                e.is_timeout(),
                e
            )
        })?;

    let status = resp.status();
    let raw: Value = resp
        .json()
        .map_err(|e| format!("json decode (tavily): {}", e))?;

    if !status.is_success() {
        return Err(format!("tavily status: {} body={}", status, raw));
    }

    // Convert the result list into compact "facts" text to pass to Mistral.
    let mut lines: Vec<String> = Vec::new();

    if let Some(results) = raw.get("results").and_then(|r| r.as_array()) {
        for r in results {
            let title = r
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
            let content = r
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();

            let mut chunk = String::new();
            if !title.is_empty() {
                chunk.push_str(title);
            }
            if !url.is_empty() {
                if !chunk.is_empty() {
                    chunk.push_str(" — ");
                }
                chunk.push_str(url);
            }
            if !content.is_empty() {
                if !chunk.is_empty() {
                    chunk.push('\n');
                }
                chunk.push_str(content);
            }

            if !chunk.is_empty() {
                lines.push(chunk);
            }
        }
    }

    if lines.is_empty() {
        return Err("tavily returned no results".into());
    }

    Ok(TavilyResult {
        raw,
        facts_text: lines.join("\n\n"),
    })
}
