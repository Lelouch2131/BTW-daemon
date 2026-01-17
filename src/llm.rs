use serde_json::Value;

pub struct LlmIntent {
    pub command_id: Option<String>,
    pub parameters: Value,
    pub confidence: f32,
}

pub trait LlmClient: Send + Sync {
    fn classify_intent(&self, text: &str, commands: &[crate::intent::IntentCommand]) -> Result<LlmIntent, String>;
    fn summarize_search(&self, query: &str, snippets: &[String]) -> Result<String, String>;
    fn answer_short(&self, prompt: &str) -> Result<String, String>;
    fn tts(&self, text: &str) -> Result<Vec<u8>, String>; // return WAV bytes
}

pub struct GroqClient { api_key: String }

impl GroqClient {
    pub fn new(api_key: String) -> Self { Self { api_key } }
}

impl LlmClient for GroqClient {
    fn classify_intent(&self, text: &str, commands: &[crate::intent::IntentCommand]) -> Result<LlmIntent, String> {
        let url = "https://api.groq.com/openai/v1/chat/completions";
        let commands_list: Vec<_> = commands.iter().map(|c| serde_json::json!({"id": c.id, "description": c.description})).collect();
        let system = "You are an intent classifier. Return ONLY a JSON object with keys: command_id, parameters, confidence. Choose the best matching command_id from the provided list or null if none.";
        let user_prompt = serde_json::json!({"text": text, "commands": commands_list}).to_string();
        let client = reqwest::blocking::Client::new();
        let req_body = serde_json::json!({
            "model": "llama-3.1-8b-instant",
            "temperature": 0.0,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user_prompt}
            ]
        });
        let resp = client.post(url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send().map_err(|e| format!("http error: {}", e))?;
        let val: Value = resp.json().map_err(|e| format!("json error: {}", e))?;
        let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("{}");
        let parsed: Value = serde_json::from_str(content).unwrap_or(serde_json::json!({"command_id": null, "parameters": {}, "confidence": 0.0}));
        Ok(LlmIntent {
            command_id: parsed["command_id"].as_str().map(|s| s.to_string()),
            parameters: parsed.get("parameters").cloned().unwrap_or(serde_json::json!({})),
            confidence: parsed["confidence"].as_f64().unwrap_or(0.0) as f32,
        })
    }

    fn summarize_search(&self, _query: &str, snippets: &[String]) -> Result<String, String> {
        let api_key = &self.api_key;
        let url = "https://api.groq.com/openai/v1/chat/completions";
        let text = if let Some(s) = snippets.first() { s } else { return Err("no snippets".into()) };
        let system = "Summarize the following answer into a few concise sentences suitable for speech. Return only the sentences.";
        let user_prompt = text;
        let req_body = serde_json::json!({
            "model": "llama-3.1-8b-instant",
            "temperature": 0.0,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user_prompt}
            ]
        });
        let client = reqwest::blocking::Client::new();
        let resp = client.post(url)
            .bearer_auth(api_key)
            .json(&req_body)
            .send().map_err(|e| format!("http error: {}", e))?;
        let val: Value = resp.json().map_err(|e| e.to_string())?;
        let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
        if content.is_empty() { Err("empty summary".into()) } else { Ok(content) }
    }

    fn answer_short(&self, prompt: &str) -> Result<String, String> {
        let api_key = &self.api_key;
        let url = "https://api.groq.com/openai/v1/chat/completions";
        let system = "You are a helpful voice assistant named Bumblebee. Answer the user's question concisely in one or two sentences. Avoid markdown; output plain text only.";
        let req_body = serde_json::json!({
            "model": "llama-3.1-8b-instant",
            "temperature": 0.2,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": prompt}
            ]
        });
        let client = reqwest::blocking::Client::new();
        let resp = client.post(url)
            .bearer_auth(api_key)
            .json(&req_body)
            .send().map_err(|e| format!("http error: {}", e))?;
        let val: Value = resp.json().map_err(|e| e.to_string())?;
        let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
        if content.trim().is_empty() { Err("empty answer".into()) } else { Ok(content.trim().to_string()) }
    }

    fn tts(&self, text: &str) -> Result<Vec<u8>, String> {
        let url = "https://api.groq.com/openai/v1/audio/speech";
        let model = std::env::var("BTWD_TTS_MODEL").unwrap_or_else(|_| "canopylabs/orpheus-v1-english".to_string());
        let voice = std::env::var("BTWD_TTS_VOICE").unwrap_or_else(|_| "alloy".to_string());
        let response_format = std::env::var("BTWD_TTS_FORMAT").unwrap_or_else(|_| "wav".to_string());
        let req_body = serde_json::json!({
            "model": model,
            "voice": voice,
            "input": text,
            "response_format": response_format,
            "speed": 1.0,
        });
        let client = reqwest::blocking::Client::new();
        let resp = client.post(url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send().map_err(|e| format!("http error: {}", e))?;
        if !resp.status().is_success() { return Err(format!("tts http status: {}", resp.status())); }
        let bytes = resp.bytes().map_err(|e| format!("read body: {}", e))?.to_vec();
        Ok(bytes)
    }
}

pub struct MistralClient { api_key: String }

impl MistralClient { pub fn new(api_key: String) -> Self { Self { api_key } } }

impl LlmClient for MistralClient {
    fn classify_intent(&self, text: &str, commands: &[crate::intent::IntentCommand]) -> Result<LlmIntent, String> {
        let url = "https://api.mistral.ai/v1/chat/completions";
        let commands_list: Vec<_> = commands.iter().map(|c| serde_json::json!({"id": c.id, "description": c.description})).collect();
        let system = "You are an intent classifier. Return ONLY a JSON object with keys: command_id, parameters, confidence. Choose the best matching command_id from the provided list or null if none.";
        let user_prompt = serde_json::json!({"text": text, "commands": commands_list}).to_string();
        let client = reqwest::blocking::Client::new();
        let req_body = serde_json::json!({
            "model": "mistral-small-latest",
            "temperature": 0.0,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user_prompt}
            ]
        });
        let resp = client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .map_err(|e| {
                format!(
                    "http error (mistral classify): connect={} timeout={} source={}",
                    e.is_connect(),
                    e.is_timeout(),
                    e
                )
            })?;
        let val: Value = resp.json().map_err(|e| format!("json error: {}", e))?;
        let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("{}");
        let parsed: Value = serde_json::from_str(content).unwrap_or(serde_json::json!({"command_id": null, "parameters": {}, "confidence": 0.0}));
        Ok(LlmIntent {
            command_id: parsed["command_id"].as_str().map(|s| s.to_string()),
            parameters: parsed.get("parameters").cloned().unwrap_or(serde_json::json!({})),
            confidence: parsed["confidence"].as_f64().unwrap_or(0.0) as f32,
        })
    }

    fn summarize_search(&self, _query: &str, snippets: &[String]) -> Result<String, String> {
        let url = "https://api.mistral.ai/v1/chat/completions";
        let text = if let Some(s) = snippets.first() { s } else { return Err("no snippets".into()) };
        let system = "Summarize the following answer into a few concise sentences suitable for speech. Return only the sentences.";
        let user_prompt = text;
        let req_body = serde_json::json!({
            "model": "mistral-small-latest",
            "temperature": 0.0,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user_prompt}
            ]
        });
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .map_err(|e| {
                format!(
                    "http error (mistral summarize): connect={} timeout={} source={}",
                    e.is_connect(),
                    e.is_timeout(),
                    e
                )
            })?;
        let val: Value = resp.json().map_err(|e| e.to_string())?;
        let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
        if content.is_empty() { Err("empty summary".into()) } else { Ok(content) }
    }

    fn answer_short(&self, prompt: &str) -> Result<String, String> {
        let url = "https://api.mistral.ai/v1/chat/completions";
        let system = "You are a helpful voice assistant named Bumblebee. Answer the user's question concisely in one or two sentences. Avoid markdown; output plain text only.";
        let req_body = serde_json::json!({
            "model": "mistral-small-latest",
            "temperature": 0.2,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": prompt}
            ]
        });
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .map_err(|e| {
                format!(
                    "http error (mistral answer): connect={} timeout={} source={}",
                    e.is_connect(),
                    e.is_timeout(),
                    e
                )
            })?;
        let val: Value = resp.json().map_err(|e| e.to_string())?;
        let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
        if content.trim().is_empty() { Err("empty answer".into()) } else { Ok(content.trim().to_string()) }
    }

    fn tts(&self, _text: &str) -> Result<Vec<u8>, String> {
        Err("Mistral TTS not supported".into())
    }
}
