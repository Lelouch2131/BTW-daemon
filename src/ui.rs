use std::process::{Command, Stdio};

fn sanitize_passive_body(body: &str) -> String {
    // Some notification daemons (e.g. swaync) may auto-add COPY actions for certain
    // bodies (especially with markup/quotes). Keep bodies plain and unquoted.
    // Do NOT touch business logic; this is display-only.
    let mut out = String::with_capacity(body.len());
    for ch in body.chars() {
        match ch {
            // Drop common quote-style characters.
            '"' | '\'' | '“' | '”' | '‘' | '’' | '`' => {}
            _ => out.push(ch),
        }
    }
    out
}

pub fn notify_listening(enabled: bool, timeout_ms: u64) {
    if !enabled { return; }
    // Fire-and-forget desktop notification using notify-send if available
    std::thread::spawn(move || {
        let _ = Command::new("notify-send")
            .arg("btwd")
            .arg("Listening…")
            .arg("-t").arg(timeout_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

pub fn notify_text(enabled: bool, timeout_ms: u64, title: &str, body: &str) {
    if !enabled { return; }
    let title = title.to_string();
    let body = sanitize_passive_body(body);
    std::thread::spawn(move || {
        let _ = Command::new("notify-send")
            .arg(title)
            .arg(body)
            // Passive/info-only notification: no actions.
            .arg("-h").arg("string:x-canonical-private-synchronous:btwd-info")
            .arg("-h").arg("string:category:im.received")
            .arg("-h").arg("int:transient:1")
            .arg("-t").arg(timeout_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

pub fn notify_answer(enabled: bool, timeout_ms: u64, title: &str, body: &str) {
    if !enabled { return; }
    let title = title.to_string();
    let body = sanitize_passive_body(body);
    std::thread::spawn(move || {
        let _ = Command::new("notify-send")
            .arg(title)
            .arg(body)
            // Passive/info-only notification: no actions.
            // Avoid "critical" urgency, which some daemons treat specially.
            .arg("-u").arg("normal")
            .arg("-h").arg("string:x-canonical-private-synchronous:btwd-answer")
            .arg("-h").arg("string:category:im.received")
            .arg("-h").arg("int:transient:1")
            .arg("-t").arg(timeout_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

pub fn notify_answer_with_open_in_browser(
    enabled: bool,
    timeout_ms: u64,
    title: &str,
    body: &str,
    google_query_url: &str,
) {
    if !enabled {
        return;
    }

    let title = title.to_string();
    let body = sanitize_passive_body(body);
    let google_query_url = google_query_url.to_string();

    std::thread::spawn(move || {
        let status = Command::new("notify-send")
            .arg(title)
            .arg(body)
            .arg("--action")
            .arg("open=Open in browser")
            .arg("-u")
            .arg("normal")
            .arg("-h")
            .arg("string:x-canonical-private-synchronous:btwd-answer")
            .arg("-h")
            .arg("string:category:im.received")
            .arg("-h")
            .arg("int:transient:1")
            .arg("-t")
            .arg(timeout_ms.to_string())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();

        let output = match status {
            Ok(o) => o,
            Err(e) => {
                eprintln!("notify-send error: {}", e);
                return;
            }
        };

        if !output.status.success() {
            eprintln!("notify-send failed: status={:?}", output.status.code());
            return;
        }

        let selection = String::from_utf8_lossy(&output.stdout);
        if selection.trim() == "open" {
            if let Err(e) = Command::new("xdg-open")
                .arg(&google_query_url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                eprintln!("xdg-open error: {}", e);
            }
        }
    });
}

pub fn notify_confirm_actions(enabled: bool, request_id: &str, title: &str, body: &str) {
    if !enabled { return; }
    let request_id = request_id.to_string();
    let title = title.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        // Use a small helper that can use dunstify actions when available.
        let helper = "./scripts/btwd-notify-confirm.sh";
        let _ = Command::new(helper)
            .arg(&request_id)
            .arg(&title)
            .arg(&body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}
